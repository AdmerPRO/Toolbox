use std::{path::PathBuf, process::Stdio};

use axum::{
    Json,
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use rand::RngExt;
use regex::Regex;
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

    let output = Command::new("yt-dlp")
        .args([
            "--dump-single-json",
            "--no-playlist",
            "--skip-download",
            "--",
        ])
        .arg(request.url.trim())
        .output()
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

    let title = video["title"].as_str().unwrap_or("YouTube video").to_owned();
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
    qualities.retain(|quality| *quality <= 2160);

    if qualities.is_empty() {
        return Err(bad_request("No MP4 versions were found for this video."));
    }

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

    let output_path = download_youtube_mp4(request.url.trim(), request.quality)
        .await
        .map_err(|error| {
            tracing::error!(%error, "YouTube download failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "The video download failed.".into(),
            )
        })?;

    let filename = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not prepare the downloaded file.".into(),
        ))?;

    Ok(Json(YoutubeDownload {
        download_url: format!("/api/youtube/file/{filename}"),
    }))
}

pub async fn download_youtube_mp4(
    url: &str,
    quality: u32,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let output_dir = PathBuf::from("storage/ytmp4");

    fs::create_dir_all(&output_dir).await?;

    // Losowa liczba od 1 do 2_147_483_646
    let id: u32 = rand::rng().random_range(1..=2_147_483_646);

    // Retrieve the video title before downloading.
    let title_output = Command::new("yt-dlp")
        .arg("--get-title")
        .arg("--")
        .arg(url)
        .output()
        .await?;

    if !title_output.status.success() {
        let error = String::from_utf8_lossy(&title_output.stderr);

        return Err(format!("Could not retrieve the video title:\n{}", error).into());
    }

    let title = String::from_utf8_lossy(&title_output.stdout)
        .trim()
        .to_string();

    // Usuwamy znaki specjalne
    let re = Regex::new(r#"[^a-zA-Z0-9ąćęłńóśźżĄĆĘŁŃÓŚŹŻ_-]+"#)?;

    let clean_title = re.replace_all(&title, "-").trim_matches('-').to_string();

    // 67 -> 0000000067
    let filename = format!("{:010}-{}.mp4", id, clean_title);

    let output_path = output_dir.join(&filename);

    let quality_selector = format!(
        "bestvideo[height<={}] + bestaudio/best[height<={}]",
        quality, quality
    );

    let output = Command::new("yt-dlp")
        .arg("-f")
        .arg(&quality_selector)
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("-o")
        .arg(&output_path)
        .arg("--")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);

        return Err(format!("yt-dlp error:\n{}", error).into());
    }

    Ok(output_path)
}

pub async fn download_file_handler(
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> Result<Response, StatusCode> {
    let path = PathBuf::from("storage/ytmp4").join(&filename);

    info!(
        endpoint = "/api/youtube/file",
        filename = %filename,
        "File requested"
    );

    if !path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let file = fs::File::open(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let metadata = file
        .metadata()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let stream = ReaderStream::new(file);

    let body = Body::from_stream(stream);

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(response)
}
