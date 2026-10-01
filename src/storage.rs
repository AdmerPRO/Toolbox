use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::Path as RoutePath,
    http::{StatusCode, header},
    response::Response,
};
use chrono::{DateTime, NaiveDate, Utc};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio_util::io::ReaderStream;

pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub fn valid_date(date: &str) -> bool {
    date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && NaiveDate::parse_from_str(date, "%d%m%Y").is_ok()
}

pub fn staging() -> Result<tempfile::TempDir> {
    fs::create_dir_all("storage/staging")?;
    Ok(tempfile::tempdir_in("storage/staging")?)
}

// Publish only completed work. The renamed job folder contains originals and results.
pub fn publish(work: &Path, output: &str) -> Result<String> {
    let date = Utc::now().format("%d%m%Y").to_string();
    let (id, _) = output.rsplit_once('.').context("Invalid output filename")?;
    let directory = Path::new("storage/active").join(&date);
    fs::create_dir_all(&directory)?;
    fs::rename(work, directory.join(id))?;
    Ok(format!("/api/files/{date}/{output}"))
}

fn valid_filename(filename: &str) -> bool {
    filename.rsplit_once('.').is_some_and(|(id, ext)| {
        uuid::Uuid::parse_str(id).is_ok()
            && matches!(ext, "png" | "jpg" | "jpeg" | "webp" | "ico" | "mp3" | "mp4")
    })
}

pub async fn download_handler(
    RoutePath((date, filename)): RoutePath<(String, String)>,
) -> Result<Response, StatusCode> {
    if !valid_date(&date) || !valid_filename(&filename) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = filename.rsplit_once('.').unwrap().0;
    let path = Path::new("storage/active")
        .join(date)
        .join(id)
        .join(&filename);
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let metadata = file
        .metadata()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if SystemTime::now()
        .duration_since(
            metadata
                .modified()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
        .unwrap_or_default()
        >= RETENTION
    {
        return Err(StatusCode::GONE);
    }
    let mime = match filename.rsplit_once('.').unwrap().1 {
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/x-icon",
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(header::CACHE_CONTROL, "private, no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub fn start_archiver() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            match tokio::task::spawn_blocking(|| {
                archive_expired(Path::new("storage"), SystemTime::now())
            })
            .await
            {
                Ok(Ok(())) => {}
                result => {
                    tracing::error!(?result, "Storage archiving failed; will retry in one hour")
                }
            }
        }
    });
}

fn archive_expired(root: &Path, now: SystemTime) -> Result<()> {
    let mut groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    // Also archive files produced by older versions of the application.
    for area in ["active", "ytmp3", "ytmp4"] {
        let directory = root.join(area);
        if !directory.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&directory).follow_links(false) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let modified = entry.metadata()?.modified()?;
            if now.duration_since(modified).unwrap_or_default() < RETENTION {
                continue;
            }
            let date = if area == "active" {
                entry
                    .path()
                    .strip_prefix(&directory)?
                    .components()
                    .next()
                    .context("Missing date folder")?
                    .as_os_str()
                    .to_string_lossy()
                    .into_owned()
            } else {
                DateTime::<Utc>::from(modified).format("%d%m%Y").to_string()
            };
            if valid_date(&date) {
                groups.entry(date).or_default().push(entry.into_path());
            }
        }
    }
    for (date, files) in groups {
        let destination = root.join("archives").join(&date);
        fs::create_dir_all(&destination)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&destination)?;
        {
            let mut writer = zip::ZipWriter::new(temporary.as_file_mut());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .large_file(true);
            for path in &files {
                let name = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                writer.start_file(name, options)?;
                std::io::copy(&mut fs::File::open(path)?, &mut writer)?;
            }
            writer.finish()?;
        }
        temporary.as_file().sync_all()?;
        // Verify every entry's CRC before removing any source files.
        {
            let mut archive = zip::ZipArchive::new(temporary.reopen()?)?;
            for index in 0..archive.len() {
                std::io::copy(&mut archive.by_index(index)?, &mut std::io::sink())?;
            }
        }
        temporary.persist(destination.join(format!("{}.zip", uuid::Uuid::new_v4())))?;
        for path in files {
            fs::remove_file(&path)?;
            // Remove empty job/day folders only; active work is never removed recursively.
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if directory == root || fs::remove_dir(directory).is_err() {
                    break;
                }
                parent = directory.parent();
            }
        }
        tracing::info!(%date, "Expired files archived");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn validate_download_paths() {
        assert!(valid_date("01102026"));
        assert!(!valid_date("31022026"));
        assert!(!valid_date("../files"));
        assert!(!valid_filename("../secret.png"));
        assert!(!valid_filename("invalid.mp3"));
    }

    #[test]
    fn archives_expired_files_and_preserves_recent_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let day = root.path().join("active/01102026/job");
        fs::create_dir_all(&day)?;
        let old = day.join("old.png");
        let recent = day.join("recent.png");
        fs::write(&old, b"original bytes")?;
        fs::write(&recent, b"recent bytes")?;
        let now = SystemTime::now();
        fs::File::options()
            .write(true)
            .open(&old)?
            .set_times(fs::FileTimes::new().set_modified(now - RETENTION))?;
        archive_expired(root.path(), now)?;
        assert!(!old.exists());
        assert!(recent.exists());
        let archive_path = fs::read_dir(root.path().join("archives/01102026"))?
            .next()
            .unwrap()?
            .path();
        let mut archive = zip::ZipArchive::new(fs::File::open(archive_path)?)?;
        assert_eq!(archive.len(), 1);
        let mut data = Vec::new();
        archive
            .by_name("active/01102026/job/old.png")?
            .read_to_end(&mut data)?;
        assert_eq!(data, b"original bytes");
        archive_expired(root.path(), now)?;
        assert_eq!(
            fs::read_dir(root.path().join("archives/01102026"))?.count(),
            1
        );
        Ok(())
    }

    #[test]
    fn archive_failure_preserves_source_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("ytmp3");
        fs::create_dir_all(&directory)?;
        let source = directory.join("old.mp3");
        fs::write(&source, b"audio bytes")?;
        let now = SystemTime::now();
        fs::File::options()
            .write(true)
            .open(&source)?
            .set_times(fs::FileTimes::new().set_modified(now - RETENTION))?;
        // A file in place of the archive directory simulates an I/O failure.
        fs::write(root.path().join("archives"), b"blocked")?;
        assert!(archive_expired(root.path(), now).is_err());
        assert_eq!(fs::read(source)?, b"audio bytes");
        Ok(())
    }
}
