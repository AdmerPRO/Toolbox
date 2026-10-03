use std::{
    path::PathBuf,
    process::{Output, Stdio},
};

static DOWNLOAD_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

const MAX_YOUTUBE_BYTES: u64 = 500 * 1024 * 1024;
const MAX_JOB_BYTES: u64 = 1500 * 1024 * 1024;

async fn run(command: &mut Command, seconds: u64) -> std::io::Result<Output> {
    crate::process::run(command, seconds, None, 0).await
}

fn download_options(command: &mut Command) -> &mut Command {
    command.args([
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-playlist",
        "--socket-timeout",
        "30",
        "--max-filesize",
        "524288000",
        "--match-filters",
        "!is_live & duration > 0 & duration <= 7200",
        "--no-progress",
        "--no-cache-dir",
        "--retries",
        "3",
        "--fragment-retries",
        "3",
        "--postprocessor-args",
        "ffmpeg:-threads 1",
    ])
}

fn storage_error(error: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::warn!(%error, "Storage unavailable");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "Storage is temporarily unavailable.".into(),
    )
}

async fn check_download(path: &std::path::Path) -> Result<(), (StatusCode, String)> {
    let size = fs::metadata(path).await.map_err(storage_error)?.len();
    if size == 0 || size > MAX_YOUTUBE_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "Downloaded file exceeds the 500 MiB limit.".into(),
        ));
    }
    Ok(())
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

fn youtube_video_url(input: &str) -> Option<String> {
    let url = url::Url::parse(input.trim()).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || !matches!(
            url.host_str(),
            Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
        )
    {
        return None;
    }
    let id = if url.host_str() == Some("youtu.be") {
        url.path().strip_prefix('/')?.to_owned()
    } else if url.path() == "/watch" {
        let mut ids = url.query_pairs().filter(|(key, _)| key == "v");
        let id = ids.next()?.1.into_owned();
        if ids.next().is_some() {
            return None;
        }
        id
    } else {
        ["/shorts/", "/embed/", "/live/"]
            .iter()
            .find_map(|prefix| url.path().strip_prefix(prefix))?
            .to_owned()
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(format!("https://www.youtube.com/watch?v={id}"))
}

fn is_youtube_url(input: &str) -> bool {
    youtube_video_url(input).is_some()
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
                "--no-plugin-dirs",
                "--no-cache-dir",
                "--socket-timeout",
                "30",
                "--dump-single-json",
                "--no-playlist",
                "--skip-download",
                "--",
            ])
            .arg(youtube_video_url(&request.url).unwrap()),
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
    if video["is_live"].as_bool() == Some(true)
        || !video["duration"]
            .as_f64()
            .is_some_and(|d| d.is_finite() && d > 0.0 && d <= 7200.0)
    {
        return Err(bad_request("Choose a recorded video up to 2 hours."));
    }
    let thumbnail = video["thumbnail"]
        .as_str()
        .filter(|value| {
            url::Url::parse(value).is_ok_and(|u| {
                u.scheme() == "https"
                    && matches!(u.host_str(), Some("i.ytimg.com" | "img.youtube.com"))
            })
        })
        .map(str::to_owned);
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
    let work = crate::storage::prepare(2 * 1024 * 1024 * 1024)
        .await
        .map_err(storage_error)?;
    let download_url = download_youtube_mp4(
        &youtube_video_url(&request.url).unwrap(),
        request.quality,
        work,
    )
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
    work: crate::storage::Staging,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let output_dir = work.path();

    let filename = format!("{}.mp4", uuid::Uuid::new_v4());
    let output_path = output_dir.join(&filename);
    let quality_selector = format!(
        "bestvideo[ext=mp4][height<={quality}]+bestaudio[ext=m4a]/best[ext=mp4][height<={quality}]"
    );

    let output = crate::process::run(
        download_options(&mut Command::new("yt-dlp"))
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
        600,
        Some(work.path()),
        MAX_JOB_BYTES,
    )
    .await?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);

        return Err(format!("yt-dlp error:\n{}", error).into());
    }

    if !fs::try_exists(&output_path).await? {
        return Err("The downloaded file was not created.".into());
    }
    check_download(&output_path)
        .await
        .map_err(|(_, message)| message)?;
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
    let work = crate::storage::prepare(2 * 1024 * 1024 * 1024)
        .await
        .map_err(storage_error)?;
    let directory = work.path();
    let filename = format!("{}.mp3", uuid::Uuid::new_v4());
    let path = directory.join(&filename);
    let output = crate::process::run(
        download_options(&mut Command::new("yt-dlp"))
            .args([
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
            .arg(youtube_video_url(&request.url).unwrap()),
        600,
        Some(work.path()),
        MAX_JOB_BYTES,
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
    check_download(&path).await?;
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
        assert!(is_youtube_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        ));
        assert!(is_youtube_url(" https://youtu.be/dQw4w9WgXcQ "));
        assert!(!is_youtube_url("https://youtube.com.evil.example/video"));
        assert!(!is_youtube_url("file:///etc/passwd"));
        for url in [
            "https://youtube.com@evil.example/x",
            "https://user@youtube.com/watch?v=x",
            "https://youtu.be:444/x",
            "http://youtube.com/watch?v=x",
            "https://youtube.com.evil.com/x",
            "https://127.0.0.1/x",
        ] {
            assert!(!is_youtube_url(url), "{url}");
        }
        assert!(is_youtube_url(
            "https://WWW.YOUTUBE.COM:443/watch?v=dQw4w9WgXcQ"
        ));
    }

    #[test]
    fn restricts_downloads_to_one_canonical_video() {
        for input in [
            "https://youtube.com/playlist?list=abc",
            "https://youtube.com/@channel",
            "https://youtu.be/dQw4w9WgXcQ/extra",
            "https://youtube.com/watch?v=dQw4w9WgXcQ&v=abcdefghijk",
        ] {
            assert!(!is_youtube_url(input), "{input}");
        }
        for input in [
            "https://youtu.be/dQw4w9WgXcQ?list=abc",
            "https://youtube.com/shorts/dQw4w9WgXcQ",
            "https://youtube.com/watch?v=dQw4w9WgXcQ&list=abc",
        ] {
            assert_eq!(
                youtube_video_url(input).as_deref(),
                Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            );
        }
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
            url: "https://youtu.be/dQw4w9WgXcQ".into(),
            quality: 999,
        }))
        .await
        .err()
        .unwrap();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
    }
}
