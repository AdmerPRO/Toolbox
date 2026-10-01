use crate::{api::download_youtube_mp4::YoutubeDownload, storage};
use axum::{
    Json,
    extract::{Multipart, Query},
    http::StatusCode,
};
use image::{DynamicImage, ImageFormat, ImageReader};
use std::{
    io::{Cursor, Write},
    path::Path,
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

pub const IMAGE_LIMIT: usize = 20 * 1024 * 1024;
pub const VIDEO_LIMIT: usize = 200 * 1024 * 1024;
static SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
type Error = (StatusCode, String);

fn bad(message: &str) -> Error {
    (StatusCode::BAD_REQUEST, message.into())
}
fn internal(error: impl std::fmt::Display) -> Error {
    tracing::error!(%error, "Media conversion failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Conversion failed. Please try again.".into(),
    )
}

fn format(extension: &str) -> Option<ImageFormat> {
    match extension {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::WebP),
        "ico" => Some(ImageFormat::Ico),
        "bmp" => Some(ImageFormat::Bmp),
        "tiff" | "tif" => Some(ImageFormat::Tiff),
        _ => None,
    }
}

async fn receive(
    mut multipart: Multipart,
    directory: &Path,
    limit: usize,
    audio: bool,
) -> Result<String, Error> {
    let mut uploaded = false;
    let mut selection = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| bad("Invalid or oversized upload."))?
    {
        match field.name() {
            Some("format") if !audio && selection.is_none() => {
                let mut bytes = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| bad("Invalid output format."))?
                {
                    if bytes.len() + chunk.len() > 16 {
                        return Err(bad("Invalid output format."));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let value = std::str::from_utf8(&bytes)
                    .map_err(|_| bad("Invalid output format."))?
                    .to_ascii_lowercase();
                if format(&value).is_none() {
                    return Err(bad("Choose PNG, JPG, JPEG, WebP, ICO, BMP, or TIFF."));
                }
                selection = Some(value);
            }
            Some("file") if !uploaded => {
                let extension = field
                    .file_name()
                    .and_then(|name| {
                        name.rsplit_once('.')
                            .map(|(_, ext)| ext.to_ascii_lowercase())
                    })
                    .ok_or_else(|| bad("Choose a file with a supported extension."))?;
                if (audio && extension != "mp4") || (!audio && format(&extension).is_none()) {
                    return Err(bad("Unsupported input file type."));
                }
                let mut file = tokio::fs::File::create(directory.join("input"))
                    .await
                    .map_err(internal)?;
                let mut size = 0usize;
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| bad("Invalid or oversized upload."))?
                {
                    size += chunk.len();
                    if size > limit {
                        return Err((
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "The selected file is too large.".into(),
                        ));
                    }
                    file.write_all(&chunk).await.map_err(internal)?;
                }
                if size == 0 {
                    return Err(bad("The selected file is empty."));
                }
                file.flush().await.map_err(internal)?;
                drop(file);
                tokio::fs::rename(
                    directory.join("input"),
                    directory.join(format!("source.{extension}")),
                )
                .await
                .map_err(internal)?;
                uploaded = true;
            }
            _ => return Err(bad("Send one file and one output format.")),
        }
    }
    if !uploaded {
        return Err(bad("Choose a file to upload."));
    }
    if audio {
        Ok("mp3".into())
    } else {
        selection.ok_or_else(|| bad("Choose an output format."))
    }
}

fn convert_image(input: &[u8], extension: &str) -> anyhow::Result<Vec<u8>> {
    convert_image_sized(input, extension, None)
}

fn convert_image_sized(
    input: &[u8],
    extension: &str,
    size: Option<(u32, u32)>,
) -> anyhow::Result<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(input)).with_guessed_format()?;
    anyhow::ensure!(
        matches!(
            reader.format(),
            Some(
                ImageFormat::Png
                    | ImageFormat::Jpeg
                    | ImageFormat::WebP
                    | ImageFormat::Ico
                    | ImageFormat::Bmp
                    | ImageFormat::Tiff
            )
        ),
        "Unsupported image contents"
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut image = reader.decode()?;
    if let Some((width, height)) = size {
        anyhow::ensure!(
            (1..=4096).contains(&width) && (1..=4096).contains(&height),
            "Invalid image dimensions"
        );
        image = image.resize(width, height, image::imageops::FilterType::Lanczos3);
    }
    if extension == "ico" {
        image = DynamicImage::ImageRgba8(image.thumbnail(256, 256).to_rgba8());
    }
    if matches!(extension, "jpg" | "jpeg") {
        // JPEG has no transparency. Composite transparent pixels onto white.
        let mut pixels = image.to_rgba8();
        for pixel in pixels.pixels_mut() {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel.0[..3] {
                *channel = ((u16::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
            }
            pixel[3] = 255;
        }
        image = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(pixels).to_rgb8());
    }
    let mut output = Cursor::new(Vec::new());
    image.write_to(
        &mut output,
        format(extension).context("Invalid output format")?,
    )?;
    Ok(output.into_inner())
}

use anyhow::Context;

pub async fn image_handler(multipart: Multipart) -> Result<Json<YoutubeDownload>, Error> {
    image_job(multipart, None).await
}

#[derive(serde::Deserialize)]
pub struct ResizeOptions {
    width: u32,
    height: u32,
}

pub async fn resize_handler(
    Query(options): Query<ResizeOptions>,
    multipart: Multipart,
) -> Result<Json<YoutubeDownload>, Error> {
    if !(1..=4096).contains(&options.width) || !(1..=4096).contains(&options.height) {
        return Err(bad("Choose width and height between 1 and 4096 pixels."));
    }
    image_job(multipart, Some((options.width, options.height))).await
}

async fn image_job(
    multipart: Multipart,
    size: Option<(u32, u32)>,
) -> Result<Json<YoutubeDownload>, Error> {
    let permit = SLOTS.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "The server is busy. Please try again shortly.".into(),
        )
    })?;
    let work = storage::staging().map_err(internal)?;
    let extension = receive(multipart, work.path(), IMAGE_LIMIT, false).await?;
    let url = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let input = std::fs::read(
            std::fs::read_dir(work.path())
                .map_err(internal)?
                .next()
                .ok_or_else(|| bad("Missing input file."))?
                .map_err(internal)?
                .path(),
        )
        .map_err(internal)?;
        let data = if size.is_some() {
            convert_image_sized(&input, &extension, size)
        } else {
            convert_image(&input, &extension)
        }
        .map_err(|_| {
            bad("Could not decode this image. Use a supported image up to 4096 x 4096 pixels.")
        })?;
        let filename = format!("{}.{}", uuid::Uuid::new_v4(), extension);
        let mut file = std::fs::File::create(work.path().join(&filename)).map_err(internal)?;
        file.write_all(&data).map_err(internal)?;
        drop(file);
        storage::publish(work.path(), &filename).map_err(internal)
    })
    .await
    .map_err(internal)??;
    Ok(Json(YoutubeDownload { download_url: url }))
}

pub async fn audio_handler(multipart: Multipart) -> Result<Json<YoutubeDownload>, Error> {
    video_job(multipart, false).await
}

pub async fn mute_handler(multipart: Multipart) -> Result<Json<YoutubeDownload>, Error> {
    video_job(multipart, true).await
}

async fn video_job(multipart: Multipart, mute: bool) -> Result<Json<YoutubeDownload>, Error> {
    let _permit = SLOTS.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "The server is busy. Please try again shortly.".into(),
        )
    })?;
    let work = storage::staging().map_err(internal)?;
    receive(multipart, work.path(), VIDEO_LIMIT, true).await?;
    let filename = format!(
        "{}.{}",
        uuid::Uuid::new_v4(),
        if mute { "mp4" } else { "mp3" }
    );
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .kill_on_drop(true)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mov",
            "-i",
        ])
        .arg(work.path().join("source.mp4"));
    if mute {
        command.args([
            "-map",
            "0:v:0",
            "-an",
            "-c:v",
            "copy",
            "-map_metadata",
            "-1",
            "-y",
        ]);
    } else {
        command.args([
            "-map",
            "0:a:0",
            "-vn",
            "-map_metadata",
            "-1",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "192k",
            "-threads",
            "1",
            "-y",
        ]);
    }
    command.arg(work.path().join(&filename));
    let output = tokio::time::timeout(Duration::from_secs(600), command.output())
        .await
        .map_err(|_| {
            (
                StatusCode::REQUEST_TIMEOUT,
                "Video processing timed out. Try a shorter video.".into(),
            )
        })?
        .map_err(|error| {
            tracing::error!(%error, "Cannot start FFmpeg");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "FFmpeg is not available on the server.".into(),
            )
        })?;
    if !output.status.success() {
        return Err(bad(if mute {
            "Could not mute this video. Choose a valid MP4 containing a video track."
        } else {
            "Could not extract audio. Choose a valid MP4 containing an audio track."
        }));
    }
    let url = storage::publish(work.path(), &filename).map_err(internal)?;
    Ok(Json(YoutubeDownload { download_url: url }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_all_supported_image_formats() {
        let source = DynamicImage::new_rgba8(300, 280);
        let mut png = Cursor::new(Vec::new());
        source.write_to(&mut png, ImageFormat::Png).unwrap();
        for extension in ["png", "jpg", "jpeg", "webp", "ico", "bmp", "tiff"] {
            let encoded = convert_image(png.get_ref(), extension).unwrap();
            let decoded = image::load_from_memory(&encoded).unwrap();
            if extension == "ico" {
                assert!(decoded.width() <= 256 && decoded.height() <= 256);
            } else {
                assert_eq!((decoded.width(), decoded.height()), (300, 280));
            }
            if extension == "jpg" {
                assert_eq!(decoded.to_rgb8().get_pixel(0, 0).0, [255, 255, 255]);
            }
            assert!(convert_image(&encoded, "png").is_ok());
        }
    }

    #[test]
    fn rejects_invalid_or_oversized_images() {
        assert!(convert_image(b"not an image", "png").is_err());
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(4097, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        assert!(convert_image(png.get_ref(), "png").is_err());
    }

    #[test]
    fn opaque_images_produce_valid_ico_files() {
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(320, 280)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let ico = convert_image(png.get_ref(), "ico").unwrap();
        let decoded = image::load_from_memory(&ico).unwrap();
        assert_eq!(decoded.width(), 256);
        assert!(decoded.height() <= 256);
    }

    #[test]
    fn resizing_preserves_aspect_ratio_and_rejects_invalid_sizes() {
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(320, 160)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let resized = convert_image_sized(png.get_ref(), "png", Some((100, 100))).unwrap();
        let decoded = image::load_from_memory(&resized).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (100, 50));
        assert!(convert_image_sized(png.get_ref(), "png", Some((0, 100))).is_err());
        assert!(convert_image_sized(png.get_ref(), "png", Some((4097, 100))).is_err());
    }
}
