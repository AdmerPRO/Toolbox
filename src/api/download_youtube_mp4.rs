use std::{
    path::PathBuf,
    process::Stdio,
};

use tokio::process::Command;

pub async fn download_youtube_mp4(
    url: &str,
    quality: u32,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let output_dir = PathBuf::from("/storage/ytmp4");

    tokio::fs::create_dir_all(&output_dir).await?;

    let output_template = output_dir.join("%(title)s.%(ext)s");

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
        .arg(output_template)
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);

        return Err(format!("yt-dlp error:\n{}", error).into());
    }

    Ok(output_dir)
}