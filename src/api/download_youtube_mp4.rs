use std::{
    path::PathBuf,
    process::{Output, Stdio},
    time::Duration,
};

static DOWNLOAD_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

async fn run(command: &mut Command, seconds: u64) -> std::io::Result<Output> {
    tokio::time::timeout(Duration::from_secs(seconds), command.output())
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Download process timed out")
        })?
}

fn download_slot() -> Result<tokio::sync::SemaphorePermit<'static>, (StatusCode, String)> {
    DOWNLOAD_SLOTS.try_acquire().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "The server is busy. Please try again shortly.".into(),
        )
    })
}

use axum::{
    Json,
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use serde::{Deserialize, Serialize};
use tokio::{fs, process::Command};
use tokio_util::io::ReaderStream;
use tracing::info;

#[derive(Deserialize)]
pub struct YoutubeUrlRequest {
    pub url: String,
}

#[derive(Deserialize)]
pub struct YoutubeDownloadRequest {
    pub url: String,
    pub quality: u32,
}

#[derive(Serialize)]
pub struct YoutubeInfo {
    pub title: String,
    pub thumbnail: Option<String>,
    pub qualities: Vec<u32>,
}

#[derive(Serialize)]
pub struct YoutubeDownload {
    pub download_url: String,
}

fn is_youtube_url(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("https://youtube.com/")
        || url.starts_with("https://www.youtube.com/")
        || url.starts_with("https://m.youtube.com/")
        || url.starts_with("https://youtu.be/")
}

fn bad_request(message: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.into())
}

pub async fn youtube_info_handler(
    Json(request): Json<YoutubeUrlRequest>,
) -> Result<Json<YoutubeInfo>, (StatusCode, String)> {
    if !is_youtube_url(&request.url) {
        return Err(bad_request("Provide a valid YouTube video link."));
    }

    let _slot = download_slot()?;
    let output = run(
        Command::new("yt-dlp")
            .kill_on_drop(true)
            .args([
                "--ignore-config",
                "--socket-timeout",
                "30",
                "--dump-single-json",
                "--no-playlist",
                "--skip-download",
                "--",
            ])
            .arg(request.url.trim()),
        90,
    )
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "yt-dlp is not installed on the server.".into(),
        )
    })?;

    if !output.status.success() {
        return Err(bad_request(
            "Could not retrieve information about this video.",
        ));
    }

    let video: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "The server received invalid video data.".into(),
        )
    })?;

    let title = video["title"]
        .as_str()
        .unwrap_or("YouTube video")
        .to_owned();
    let thumbnail = video["thumbnail"].as_str().map(str::to_owned);
    let mut qualities = video["formats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|format| {
            (format["vcodec"].as_str()? != "none")
                .then(|| format["height"].as_u64())
                .flatten()
                .map(|height| height as u32)
        })
        .collect::<Vec<_>>();

    qualities.sort_unstable();
    qualities.dedup();
    qualities.retain(|quality| (144..=2160).contains(quality));

    Ok(Json(YoutubeInfo {
        title,
        thumbnail,
        qualities,
    }))
}

pub async fn youtube_download_handler(
    Json(request): Json<YoutubeDownloadRequest>,
) -> Result<Json<YoutubeDownload>, (StatusCode, String)> {
    if !is_youtube_url(&request.url) {
        return Err(bad_request("Provide a valid YouTube video link."));
    }

    if !(144..=2160).contains(&request.quality) {
        return Err(bad_request("Choose a valid video quality."));
    }

    let _slot = download_slot()?;
    let download_url = download_youtube_mp4(request.url.trim(), request.quality)
        .await
        .map_err(|error| {
            tracing::error!(%error, "YouTube download failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "The video download failed.".into(),
            )
        })?;

    Ok(Json(YoutubeDownload { download_url }))
}

pub async fn download_youtube_mp4(
    url: &str,
    quality: u32,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let work = crate::storage::staging()?;
    let output_dir = work.path();

    let filename = format!("{}.mp4", uuid::Uuid::new_v4());
    let output_path = output_dir.join(&filename);
    let quality_selector = format!(
        "bestvideo[ext=mp4][height<={quality}]+bestaudio[ext=m4a]/best[ext=mp4][height<={quality}]"
    );

    let output = run(
        Command::new("yt-dlp")
            .kill_on_drop(true)
            .args(["--ignore-config", "--no-playlist", "--socket-timeout", "30"])
            .arg("-f")
            .arg(&quality_selector)
            .arg("--merge-output-format")
            .arg("mp4")
            .arg("-o")
            .arg(&output_path)
            .arg("--")
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        1800,
    )
    .await?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);

        return Err(format!("yt-dlp error:\n{}", error).into());
    }

    if !fs::try_exists(&output_path).await? {
        return Err("The downloaded file was not created.".into());
    }
    Ok(crate::storage::publish(work.path(), &filename)?)
}

pub async fn download_file_handler(
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> Result<Response, StatusCode> {
    let (id, extension) = filename.rsplit_once('.').ok_or(StatusCode::BAD_REQUEST)?;
    if uuid::Uuid::parse_str(id).is_err() || !matches!(extension, "mp4" | "mp3") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let directory = if extension == "mp3" {
        "storage/ytmp3"
    } else {
        "storage/ytmp4"
    };
    let path = PathBuf::from(directory).join(&filename);

    info!(
        endpoint = "/api/youtube/file",
        filename = %filename,
        "File requested"
    );

    let file = fs::File::open(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let metadata = file
        .metadata()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if std::time::SystemTime::now()
        .duration_since(
            metadata
                .modified()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
        .unwrap_or_default()
        >= crate::storage::RETENTION
    {
        return Err(StatusCode::GONE);
    }

    let stream = ReaderStream::new(file);

    let body = Body::from_stream(stream);

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            if extension == "mp3" {
                "audio/mpeg"
            } else {
                "video/mp4"
            },
        )
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(response)
}

#[derive(Deserialize)]
pub struct YoutubeAudioRequest {
    pub url: String,
    pub quality: u32,
}

pub async fn youtube_mp3_handler(
    Json(request): Json<YoutubeAudioRequest>,
) -> Result<Json<YoutubeDownload>, (StatusCode, String)> {
    if !is_youtube_url(&request.url) {
        return Err(bad_request("Provide a valid YouTube video link."));
    }
    if !matches!(request.quality, 128 | 192 | 256 | 320) {
        return Err(bad_request("Choose 128, 192, 256 or 320 kbps."));
    }
    let _slot = download_slot()?;
    let work = crate::storage::staging().map_err(|error| {
        tracing::error!(%error, "Cannot create audio directory");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not prepare audio storage.".into(),
        )
    })?;
    let directory = work.path();
    let filename = format!("{}.mp3", uuid::Uuid::new_v4());
    let path = directory.join(&filename);
    let output = run(
        Command::new("yt-dlp")
            .kill_on_drop(true)
            .args([
                "--ignore-config",
                "--no-playlist",
                "--socket-timeout",
                "30",
                "-f",
                "bestaudio/best",
                "--extract-audio",
                "--audio-format",
                "mp3",
                "--audio-quality",
            ])
            .arg(format!("{}K", request.quality))
            .arg("-o")
            .arg(&path)
            .arg("--")
            .arg(request.url.trim()),
        1800,
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "Cannot start yt-dlp");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "yt-dlp is not available on the server.".into(),
        )
    })?;
    if !output.status.success() || !fs::try_exists(&path).await.unwrap_or(false) {
        tracing::error!(stderr = %String::from_utf8_lossy(&output.stderr), "Audio download failed");
        return Err((StatusCode::INTERNAL_SERVER_ERROR,
            "Audio download failed. Check that yt-dlp and FFmpeg are installed and the video is available.".into()));
    }
    Ok(Json(YoutubeDownload {
        download_url: crate::storage::publish(work.path(), &filename).map_err(|error| {
            tracing::error!(%error, "Cannot publish audio");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not store the audio file.".into(),
            )
        })?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_links_only() {
        assert!(is_youtube_url("https://www.youtube.com/watch?v=abc"));
        assert!(is_youtube_url(" https://youtu.be/abc "));
        assert!(!is_youtube_url("https://youtube.com.evil.example/video"));
        assert!(!is_youtube_url("file:///etc/passwd"));
    }

    #[tokio::test]
    async fn reject_path_traversal() {
        for name in ["../secret.mp4", "..\\secret.mp3", "invalid.mp4", "test.txt"] {
            assert_eq!(
                download_file_handler(axum::extract::Path(name.into()))
                    .await
                    .unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[tokio::test]
    async fn reject_invalid_audio_quality() {
        let error = youtube_mp3_handler(Json(YoutubeAudioRequest {
            url: "https://youtu.be/abc".into(),
            quality: 999,
        }))
        .await
        .err()
        .unwrap();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
    }
}
