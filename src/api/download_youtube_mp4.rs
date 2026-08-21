use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::Response,
};
use tokio::{fs, process::Command};
use tokio_util::io::ReaderStream;
use tracing::info;
use rand::RngExt;
use regex::Regex;

pub async fn download_youtube_mp4(
    url: &str,
    quality: u32,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let output_dir = PathBuf::from("storage/ytmp4");

    fs::create_dir_all(&output_dir).await?;

    // Losowa liczba od 1 do 2_147_483_646
    let id: u32 = rand::rng().random_range(1..=2_147_483_646);

    // Pobieramy tytuł filmu przed pobraniem
    let title_output = Command::new("yt-dlp")
        .arg("--get-title")
        .arg(url)
        .output()
        .await?;

    if !title_output.status.success() {
        let error = String::from_utf8_lossy(&title_output.stderr);

        return Err(format!("Nie udało się pobrać tytułu:\n{}", error).into());
    }

    let title = String::from_utf8_lossy(&title_output.stdout)
        .trim()
        .to_string();

    // Usuwamy znaki specjalne
    let re = Regex::new(r#"[^a-zA-Z0-9ąćęłńóśźżĄĆĘŁŃÓŚŹŻ_-]+"#)?;

    let clean_title = re
        .replace_all(&title, "-")
        .trim_matches('-')
        .to_string();

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