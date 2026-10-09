use std::{
    path::PathBuf,
    process::{Output, Stdio},
};

static INFO_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
static INFO_CACHE: std::sync::LazyLock<std::sync::Mutex<InfoCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(InfoCache::default()));
#[derive(Default)]
struct InfoCache(std::collections::HashMap<String, (std::time::Instant, YoutubeInfo)>);
impl InfoCache {
    fn get(&self, url: &str, now: std::time::Instant) -> Option<YoutubeInfo> {
        self.0
            .get(url)
            .filter(|(saved, _)| now.duration_since(*saved) < std::time::Duration::from_secs(300))
            .map(|(_, info)| info.clone())
    }
    fn insert(&mut self, url: String, info: YoutubeInfo, now: std::time::Instant) {
        self.0.retain(|_, (saved, _)| {
            now.duration_since(*saved) < std::time::Duration::from_secs(300)
        });
        if self.0.len() >= 128
            && !self.0.contains_key(&url)
            && let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, (saved, _))| *saved)
                .map(|(key, _)| key.clone())
        {
            self.0.remove(&oldest);
        }
        self.0.insert(url, (now, info));
    }
}

const MAX_YOUTUBE_BYTES: u64 = 500 * 1024 * 1024;
const MAX_JOB_BYTES: u64 = 1500 * 1024 * 1024;

async fn run(command: &mut Command, seconds: u64) -> std::io::Result<Output> {
    run_logged(command, seconds, None, 0).await
}

async fn run_logged(
    command: &mut Command,
    seconds: u64,
    directory: Option<&std::path::Path>,
    max_bytes: u64,
) -> std::io::Result<Output> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    let started = std::time::Instant::now();
    info!(tool = %program, timeout_seconds = seconds, "Media process started");
    let output = crate::process::run(command, seconds, directory, max_bytes).await;
    match &output {
        Ok(output) => {
            info!(tool = %program, exit_code = ?output.status.code(),
                elapsed_ms = started.elapsed().as_millis(), stdout_bytes = output.stdout.len(),
                stderr_bytes = output.stderr.len(), "Media process finished");
            if !output.stderr.is_empty() {
                let details: String = String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(4000)
                    .collect();
                tracing::warn!(tool = %program, stderr = %details, "Media process diagnostics (first 4000 characters)");
            }
        }
        Err(error) => tracing::error!(tool = %program, %error,
            elapsed_ms = started.elapsed().as_millis(), "Media process could not complete"),
    }
    output
}

fn download_options(command: &mut Command) -> &mut Command {
    command.args([
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-playlist",
        "--playlist-end",
        "1",
        "--socket-timeout",
        "30",
        "--max-filesize",
        "524288000",
        "--match-filters",
        "!is_live & duration >? 0 & duration <=? 7200",
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
    info!(file = %path.display(), bytes = size, "Checking downloaded media");
    if size == 0 || size > MAX_YOUTUBE_BYTES {
        tracing::warn!(
            bytes = size,
            limit_bytes = MAX_YOUTUBE_BYTES,
            "Downloaded media rejected: invalid file size"
        );
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            "Downloaded file exceeds the 500 MiB limit.".into(),
        ));
    }
    // Instagram may omit duration in metadata. Validate the actual output before publishing.
    let output = run(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "json",
            ])
            .arg(path),
        30,
    )
    .await
    .map_err(storage_error)?;
    let duration = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|data| data["format"]["duration"].as_str()?.parse::<f64>().ok());
    if !output.status.success() || !duration.is_some_and(valid_duration) {
        tracing::warn!(file = %path.display(), duration_seconds = ?duration, "Downloaded media rejected: invalid duration");
        return Err(bad_request("Choose a recorded video up to 2 hours."));
    }
    info!(file = %path.display(), bytes = size, duration_seconds = ?duration, "Downloaded media validated");
    Ok(())
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

#[derive(Clone, Serialize)]
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

fn media_video_url(input: &str) -> Option<String> {
    if let Some(url) = youtube_video_url(input) {
        return Some(url);
    }
    let url = url::Url::parse(input.trim()).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return None;
    }
    let segments: Vec<_> = url.path().trim_matches('/').split('/').collect();
    let token = |value: &str| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    match url.host_str()? {
        "instagram.com" | "www.instagram.com" => {
            let post = match segments.as_slice() {
                [kind, id] => Some((*kind, *id)),
                [username, kind, id]
                    if !username.is_empty()
                        && username.len() <= 30
                        && username
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.') =>
                {
                    Some((*kind, *id))
                }
                _ => None,
            };
            let (kind, id) = post?;
            if !matches!(kind, "p" | "reel" | "reels" | "tv") || !token(id) {
                return None;
            }
            let kind = if kind == "reels" { "reel" } else { kind };
            Some(format!("https://www.instagram.com/{kind}/{id}/"))
        }

        "tiktok.com" | "www.tiktok.com" | "m.tiktok.com" => {
            if segments.len() == 3
                && segments[0].starts_with('@')
                && segments[0][1..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
                && segments[0].len() > 1
                && segments[1] == "video"
                && !segments[2].is_empty()
                && segments[2].len() <= 32
                && segments[2].bytes().all(|b| b.is_ascii_digit())
            {
                Some(format!(
                    "https://www.tiktok.com/{}/video/{}",
                    segments[0], segments[2]
                ))
            } else if segments.len() == 2 && segments[0] == "t" && token(segments[1]) {
                Some(format!("https://www.tiktok.com/t/{}/", segments[1]))
            } else {
                None
            }
        }
        "vm.tiktok.com" | "vt.tiktok.com" if segments.len() == 1 && token(segments[0]) => {
            Some(format!("https://{}/{}/", url.host_str()?, segments[0]))
        }
        _ => None,
    }
}

fn valid_duration(seconds: f64) -> bool {
    seconds.is_finite() && seconds > 0.0 && seconds <= 7200.0
}

fn recorded_video_metadata(video: &serde_json::Value) -> bool {
    video["is_live"].as_bool() != Some(true)
        && !matches!(
            video["live_status"].as_str(),
            Some("is_live" | "is_upcoming")
        )
        && (video["duration"].is_null() || video["duration"].as_f64().is_some_and(valid_duration))
}

fn permitted_thumbnail(input: &str) -> bool {
    url::Url::parse(input).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.host_str().is_some_and(|host| {
                [
                    "i.ytimg.com",
                    "img.youtube.com",
                    "tiktokcdn.com",
                    "tiktokcdn-us.com",
                    "tiktokcdn-eu.com",
                    "tiktok.com",
                    "ibytedtos.com",
                    "byteoversea.com",
                    "cdninstagram.com",
                    "fbcdn.net",
                ]
                .iter()
                .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
            })
    })
}

fn video_thumbnail(video: &serde_json::Value) -> Option<String> {
    video["thumbnail"]
        .as_str()
        .filter(|value| permitted_thumbnail(value))
        .or_else(|| {
            video["thumbnails"]
                .as_array()?
                .iter()
                .rev()
                .filter_map(|thumbnail| thumbnail["url"].as_str())
                .find(|value| permitted_thumbnail(value))
        })
        .map(str::to_owned)
}

#[cfg(test)]
fn is_youtube_url(input: &str) -> bool {
    youtube_video_url(input).is_some()
}

fn bad_request(message: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.into())
}

#[tracing::instrument(skip_all, fields(operation = "info", request_id = %uuid::Uuid::new_v4()))]
pub async fn youtube_info_handler(
    Json(request): Json<YoutubeUrlRequest>,
) -> Result<Json<YoutubeInfo>, (StatusCode, String)> {
    info!("Media video info requested");
    if media_video_url(&request.url).is_none() {
        tracing::warn!("Media request rejected: unsupported or malformed video URL");
        return Err(bad_request(
            "Provide a valid YouTube, Instagram or TikTok video link.",
        ));
    }

    let url = media_video_url(&request.url).unwrap();
    info!(url = %url, "Video URL accepted");
    if let Some(info) = INFO_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&url, std::time::Instant::now())
    {
        info!(url = %url, "Video metadata cache hit");
        return Ok(Json(info));
    }
    info!(url = %url, "Video metadata cache miss");
    let _info_slot = INFO_SLOT.try_acquire().map_err(|_| {
        tracing::warn!("Video metadata lookup rejected: lookup slot busy");
        (
            StatusCode::TOO_MANY_REQUESTS,
            "Video information lookup is busy. Please try again shortly.".into(),
        )
    })?;
    let _slot = crate::resources::acquire()?;
    let output = run(
        Command::new("yt-dlp")
            .kill_on_drop(true)
            .args([
                "--ignore-config",
                "--no-plugin-dirs",
                "--no-cache-dir",
                "--socket-timeout",
                "15",
                "--retries",
                "1",
                "--extractor-retries",
                "1",
                "--dump-single-json",
                "--no-playlist",
                "--playlist-end",
                "1",
                "--skip-download",
                "--",
            ])
            .arg(media_video_url(&request.url).unwrap()),
        30,
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "Video metadata process failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "yt-dlp is not installed on the server.".into(),
        )
    })?;

    if !output.status.success() {
        return Err(bad_request(
            "Could not retrieve this video. Use a public video from YouTube, Instagram or TikTok that is available without login.",
        ));
    }

    let video: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        tracing::error!(%error, "Video metadata JSON is invalid");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "The server received invalid video data.".into(),
        )
    })?;

    if matches!(video["_type"].as_str(), Some("playlist" | "multi_video")) {
        return Err(bad_request(
            "Choose a link to one video, not an album or playlist.",
        ));
    }
    let title = video["title"].as_str().unwrap_or("Video").to_owned();
    if !recorded_video_metadata(&video) {
        tracing::warn!(duration = ?video["duration"], is_live = ?video["is_live"], live_status = ?video["live_status"], "Video metadata rejected");
        return Err(bad_request("Choose a recorded video up to 2 hours."));
    }
    if video["duration"].is_null() {
        info!("Metadata duration unavailable; duration will be checked after download");
    }
    let thumbnail = video_thumbnail(&video);
    if thumbnail.is_none() {
        tracing::warn!(
            thumbnail_present = video["thumbnail"].is_string(),
            candidates = video["thumbnails"].as_array().map_or(0, Vec::len),
            "No permitted thumbnail available"
        );
    }
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

    if let Some(height) = video["height"]
        .as_u64()
        .and_then(|height| u32::try_from(height).ok())
    {
        qualities.push(height);
    }
    qualities.sort_unstable();
    qualities.dedup();
    qualities.retain(|quality| (144..=2160).contains(quality));

    info!(url = %url, title = %title, duration_seconds = ?video["duration"].as_f64(), qualities = ?qualities, has_thumbnail = thumbnail.is_some(), "Video metadata ready");
    let info = YoutubeInfo {
        title,
        thumbnail,
        qualities,
    };
    INFO_CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(
        url,
        info.clone(),
        std::time::Instant::now(),
    );
    Ok(Json(info))
}

#[tracing::instrument(skip_all, fields(operation = "mp4", request_id = %uuid::Uuid::new_v4()))]
pub async fn youtube_download_handler(
    axum::Extension(crate::rate_limit::ClientIp(uploader_ip)): axum::Extension<
        crate::rate_limit::ClientIp,
    >,
    Json(request): Json<YoutubeDownloadRequest>,
) -> Result<Json<YoutubeDownload>, (StatusCode, String)> {
    info!(quality = request.quality, "Media MP4 download requested");
    if media_video_url(&request.url).is_none() {
        tracing::warn!("Media request rejected: unsupported or malformed video URL");
        return Err(bad_request(
            "Provide a valid YouTube, Instagram or TikTok video link.",
        ));
    }

    if !(144..=2160).contains(&request.quality) {
        tracing::warn!(
            quality = request.quality,
            "MP4 request rejected: invalid quality"
        );
        return Err(bad_request("Choose a valid video quality."));
    }

    let _slot = crate::resources::acquire()?;
    let work = crate::storage::prepare(2 * 1024 * 1024 * 1024)
        .await
        .map_err(storage_error)?;
    let download_url = download_youtube_mp4(
        &media_video_url(&request.url).unwrap(),
        request.quality,
        work,
        uploader_ip,
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "Media MP4 download failed");
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
    uploader_ip: std::net::IpAddr,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let output_dir = work.path();

    let filename = format!("{}.mp4", uuid::Uuid::new_v4());
    let output_path = output_dir.join(&filename);
    info!(%url, %filename, quality, "Preparing MP4 download");
    let quality_selector = format!(
        "bestvideo[ext=mp4][height<={quality}]+bestaudio[ext=m4a]/best[ext=mp4][height<={quality}]"
    );

    let output = run_logged(
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
    let download_url = crate::storage::publish_youtube(work, filename, uploader_ip).await?;
    info!(%download_url, "MP4 download ready");
    Ok(download_url)
}

pub async fn download_file_handler(
    axum::extract::Path(filename): axum::extract::Path<String>,
    axum::Extension(crate::rate_limit::ClientIp(viewer_ip)): axum::Extension<
        crate::rate_limit::ClientIp,
    >,
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
        "File id: {id} download requested"
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

    crate::audit::access(id.to_owned(), viewer_ip)
        .await
        .map_err(|error| {
            tracing::error!(%error, "Cannot audit legacy download");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
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

#[tracing::instrument(skip_all, fields(operation = "mp3", request_id = %uuid::Uuid::new_v4()))]
pub async fn youtube_mp3_handler(
    axum::Extension(crate::rate_limit::ClientIp(uploader_ip)): axum::Extension<
        crate::rate_limit::ClientIp,
    >,
    Json(request): Json<YoutubeAudioRequest>,
) -> Result<Json<YoutubeDownload>, (StatusCode, String)> {
    info!(
        bitrate_kbps = request.quality,
        "Media MP3 download requested"
    );
    if media_video_url(&request.url).is_none() {
        tracing::warn!("Media request rejected: unsupported or malformed video URL");
        return Err(bad_request(
            "Provide a valid YouTube, Instagram or TikTok video link.",
        ));
    }
    if !matches!(request.quality, 128 | 192 | 256 | 320) {
        tracing::warn!(
            bitrate_kbps = request.quality,
            "MP3 request rejected: invalid bitrate"
        );
        return Err(bad_request("Choose 128, 192, 256 or 320 kbps."));
    }
    let _slot = crate::resources::acquire()?;
    let work = crate::storage::prepare(2 * 1024 * 1024 * 1024)
        .await
        .map_err(storage_error)?;
    let directory = work.path();
    let filename = format!("{}.mp3", uuid::Uuid::new_v4());
    let path = directory.join(&filename);
    info!(url = %media_video_url(&request.url).unwrap(), %filename, bitrate_kbps = request.quality, "Preparing MP3 download");
    let output = run_logged(
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
            .arg(media_video_url(&request.url).unwrap()),
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
        download_url: crate::storage::publish_youtube(work, filename, uploader_ip)
            .await
            .map_err(|error| {
                tracing::error!(%error, "Cannot publish audio");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not store the audio file.".into(),
                )
            })
            .map(|download_url| {
                info!(%download_url, "MP3 download ready");
                download_url
            })?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_cache_expires_and_bounds_entries() {
        let mut cache = InfoCache::default();
        let start = std::time::Instant::now();
        for index in 0..129 {
            cache.insert(
                index.to_string(),
                YoutubeInfo {
                    title: index.to_string(),
                    thumbnail: None,
                    qualities: vec![720],
                },
                start + std::time::Duration::from_millis(index),
            );
        }
        assert_eq!(cache.0.len(), 128);
        assert!(
            cache
                .get("0", start + std::time::Duration::from_secs(1))
                .is_none()
        );
        assert_eq!(
            cache
                .get("128", start + std::time::Duration::from_secs(1))
                .unwrap()
                .qualities,
            vec![720]
        );
        assert!(
            cache
                .get("128", start + std::time::Duration::from_secs(301))
                .is_none()
        );
    }

    #[test]
    fn missing_metadata_duration_is_checked_after_download() {
        for video in [
            serde_json::json!({}),
            serde_json::json!({"duration": null}),
            serde_json::json!({"duration": 15}),
            serde_json::json!({"duration": 7200}),
        ] {
            assert!(recorded_video_metadata(&video));
        }
        for video in [
            serde_json::json!({"duration": 7201}),
            serde_json::json!({"duration": 0}),
            serde_json::json!({"duration": -1}),
            serde_json::json!({"duration": "invalid"}),
            serde_json::json!({"is_live": true}),
            serde_json::json!({"live_status": "is_upcoming"}),
        ] {
            assert!(!recorded_video_metadata(&video));
        }
        assert!(!valid_duration(f64::NAN));
        assert!(!valid_duration(f64::INFINITY));
        assert!(!valid_duration(7201.0));
        assert!(valid_duration(15.0));
    }

    #[test]
    fn social_thumbnails_accept_cdn_hosts_and_fallback() {
        for url in [
            "https://p16-sign.tiktokcdn-us.com/cover.jpg?signature=abc",
            "https://p16.tiktokcdn.com/cover.jpg",
            "https://scontent.cdninstagram.com/cover.jpg",
        ] {
            assert!(permitted_thumbnail(url));
            assert_eq!(
                video_thumbnail(&serde_json::json!({"thumbnail": url})).as_deref(),
                Some(url)
            );
        }
        for url in [
            "https://tiktokcdn.com.evil.example/cover.jpg",
            "http://p16.tiktokcdn.com/cover.jpg",
            "https://user@p16.tiktokcdn.com/cover.jpg",
            "https://127.0.0.1/cover.jpg",
        ] {
            assert!(!permitted_thumbnail(url));
        }
        let video = serde_json::json!({"thumbnail": "https://evil.example/a.jpg",
            "thumbnails": [{"url": "https://p16.tiktokcdn.com/cover.jpg"}]});
        assert_eq!(
            video_thumbnail(&video).as_deref(),
            Some("https://p16.tiktokcdn.com/cover.jpg")
        );
    }

    #[test]
    fn social_video_links_are_canonical_and_restricted() {
        for (input, expected) in [
            (
                "https://www.instagram.com/minecraft/reel/ABC_123/?igsh=abc",
                "https://www.instagram.com/reel/ABC_123/",
            ),
            (
                "https://instagram.com/some.user/p/ABC-123/",
                "https://www.instagram.com/p/ABC-123/",
            ),
            (
                "https://www.instagram.com/reel/ABC_123/?igsh=abc",
                "https://www.instagram.com/reel/ABC_123/",
            ),
            (
                "https://instagram.com/p/ABC-123/",
                "https://www.instagram.com/p/ABC-123/",
            ),
            (
                "https://m.tiktok.com/@some.user/video/123456?share=1",
                "https://www.tiktok.com/@some.user/video/123456",
            ),
            (
                "https://vm.tiktok.com/ABC123/?share=1",
                "https://vm.tiktok.com/ABC123/",
            ),
            (
                "https://vt.tiktok.com/ABC123/",
                "https://vt.tiktok.com/ABC123/",
            ),
            (
                "https://www.tiktok.com/t/ABC123/",
                "https://www.tiktok.com/t/ABC123/",
            ),
        ] {
            assert_eq!(media_video_url(input).as_deref(), Some(expected));
        }
        for input in [
            "https://instagram.com/profile/",
            "https://instagram.com/bad%20user/reel/abc/",
            "https://instagram.com/minecraft/reel/abc/extra",
            "https://tiktok.com/@user",
            "https://instagram.com.evil.example/reel/abc/",
            "https://user@instagram.com/reel/abc/",
            "http://vm.tiktok.com/abc/",
            "https://tiktok.com:444/@user/video/123",
            "https://www.tiktok.com/@user/photo/123",
            "https://instagram.com/reel/abc/extra",
            "https://vm.tiktok.com/abc/extra",
            "https://127.0.0.1/reel/abc/",
        ] {
            assert!(media_video_url(input).is_none(), "{input}");
        }
    }

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
                download_file_handler(
                    axum::extract::Path(name.into()),
                    axum::Extension(crate::rate_limit::ClientIp("192.0.2.1".parse().unwrap()))
                )
                .await
                .unwrap_err(),
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[tokio::test]
    async fn reject_invalid_audio_quality() {
        let error = youtube_mp3_handler(
            axum::Extension(crate::rate_limit::ClientIp("192.0.2.1".parse().unwrap())),
            Json(YoutubeAudioRequest {
                url: "https://youtu.be/dQw4w9WgXcQ".into(),
                quality: 999,
            }),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
    }
}
