use crate::{api::download_youtube_mp4::YoutubeDownload, storage};
use axum::{
    Extension, Json,
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
    multipart: Multipart,
    directory: &Path,
    limit: usize,
    audio: bool,
) -> Result<String, Error> {
    let seconds = std::env::var("UPLOAD_TIMEOUT_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    tokio::time::timeout(
        Duration::from_secs(seconds),
        receive_inner(multipart, directory, limit, audio),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::REQUEST_TIMEOUT,
            "Upload timed out. Please try again.".into(),
        )
    })?
}

async fn receive_inner(
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

pub async fn image_handler(
    Extension(client_permit): Extension<Arc<crate::rate_limit::JobPermit>>,
    Extension(crate::rate_limit::ClientIp(uploader_ip)): Extension<crate::rate_limit::ClientIp>,
    multipart: Multipart,
) -> Result<Json<YoutubeDownload>, Error> {
    image_job(multipart, None, client_permit, uploader_ip).await
}

#[derive(serde::Deserialize)]
pub struct ResizeOptions {
    width: u32,
    height: u32,
}

pub async fn resize_handler(
    Query(options): Query<ResizeOptions>,
    Extension(client_permit): Extension<Arc<crate::rate_limit::JobPermit>>,
    Extension(crate::rate_limit::ClientIp(uploader_ip)): Extension<crate::rate_limit::ClientIp>,
    multipart: Multipart,
) -> Result<Json<YoutubeDownload>, Error> {
    if !(1..=4096).contains(&options.width) || !(1..=4096).contains(&options.height) {
        return Err(bad("Choose width and height between 1 and 4096 pixels."));
    }
    image_job(
        multipart,
        Some((options.width, options.height)),
        client_permit,
        uploader_ip,
    )
    .await
}

async fn image_job(
    multipart: Multipart,
    size: Option<(u32, u32)>,
    client_permit: Arc<crate::rate_limit::JobPermit>,
    uploader_ip: std::net::IpAddr,
) -> Result<Json<YoutubeDownload>, Error> {
    let permit = SLOTS.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "The server is busy. Please try again shortly.".into(),
        )
    })?;
    let work = storage::prepare(768 * 1024 * 1024).await.map_err(|error| {
        tracing::warn!(%error, "Storage unavailable");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Storage is temporarily unavailable.".into(),
        )
    })?;
    let extension = receive(multipart, work.path(), IMAGE_LIMIT, false).await?;
    let url = tokio::task::spawn_blocking(move || {
        let _client_permit = client_permit;
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
        storage::publish(&work, &filename, uploader_ip).map_err(internal)
    })
    .await
    .map_err(internal)??;
    Ok(Json(YoutubeDownload { download_url: url }))
}

pub async fn audio_handler(
    Extension(crate::rate_limit::ClientIp(uploader_ip)): Extension<crate::rate_limit::ClientIp>,
    multipart: Multipart,
) -> Result<Json<YoutubeDownload>, Error> {
    video_job(multipart, false, uploader_ip).await
}

pub async fn mute_handler(
    Extension(crate::rate_limit::ClientIp(uploader_ip)): Extension<crate::rate_limit::ClientIp>,
    multipart: Multipart,
) -> Result<Json<YoutubeDownload>, Error> {
    video_job(multipart, true, uploader_ip).await
}

async fn video_job(
    multipart: Multipart,
    mute: bool,
    uploader_ip: std::net::IpAddr,
) -> Result<Json<YoutubeDownload>, Error> {
    let _permit = SLOTS.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "The server is busy. Please try again shortly.".into(),
        )
    })?;
    let work = storage::prepare(768 * 1024 * 1024).await.map_err(|error| {
        tracing::warn!(%error, "Storage unavailable");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Storage is temporarily unavailable.".into(),
        )
    })?;
    receive(multipart, work.path(), VIDEO_LIMIT, true).await?;
    probe_video(&work.path().join("source.mp4"), mute).await?;
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
    command
        .args(["-t", "7200", "-fs", "524288000"])
        .arg(work.path().join(&filename));
    let output = crate::process::run(&mut command, 600, Some(work.path()), 768 * 1024 * 1024)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Video processing failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Video processing failed or exceeded resource limits.".into(),
            )
        })?;
    if !output.status.success() {
        return Err(bad(if mute {
            "Could not mute this video. Choose a valid MP4 containing a video track."
        } else {
            "Could not extract audio. Choose a valid MP4 containing an audio track."
        }));
    }
    let url = storage::publish(&work, &filename, uploader_ip).map_err(internal)?;
    Ok(Json(YoutubeDownload { download_url: url }))
}

fn valid_probe(video: &serde_json::Value, mute: bool) -> bool {
    let format = &video["format"];
    let duration = format["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok());
    let streams = video["streams"].as_array();
    format["format_name"]
        .as_str()
        .is_some_and(|s| s.split(',').any(|f| f == "mp4" || f == "mov"))
        && duration.is_some_and(|d| d.is_finite() && d > 0.0 && d <= 7200.0)
        && streams.is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 10
                && s.iter().any(|stream| {
                    stream["codec_type"].as_str() == Some(if mute { "video" } else { "audio" })
                })
        })
}

async fn probe_video(path: &Path, mute: bool) -> Result<(), Error> {
    let mut command = tokio::process::Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mov",
            "-show_entries",
            "format=format_name,duration:stream=codec_type",
            "-of",
            "json",
        ])
        .arg(path);
    let output = crate::process::run(&mut command, 20, None, 0)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "ffprobe failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Video validation failed. Check that ffprobe is installed.".into(),
            )
        })?;
    let data = serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
    if !output.status.success() || !valid_probe(&data, mute) {
        return Err(bad(
            "Choose a valid MP4 up to 2 hours, with at most 10 streams and the required track.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_container_duration_stream_count_and_required_track() {
        let mut video = serde_json::json!({"format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "7200"}, "streams": [{"codec_type": "video"}]});
        assert!(valid_probe(&video, true));
        assert!(!valid_probe(&video, false));
        for duration in ["7201", "0", "-1", "NaN", "inf", "unknown"] {
            video["format"]["duration"] = duration.into();
            assert!(!valid_probe(&video, true));
        }
        video["format"]["duration"] = "1".into();
        video["streams"] = serde_json::json!([{"codec_type": "audio"}]);
        assert!(valid_probe(&video, false));
        video["streams"] = serde_json::json!(vec![serde_json::json!({"codec_type": "audio"}); 11]);
        assert!(!valid_probe(&video, false));
        video["streams"] = serde_json::json!([{"codec_type": "audio"}]);
        video["format"]["format_name"] = "matroska".into();
        assert!(!valid_probe(&video, false));
    }

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
