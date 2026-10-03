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
pub const ARCHIVE_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

pub fn valid_date(date: &str) -> bool {
    date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
        && NaiveDate::parse_from_str(date, "%d%m%Y").is_ok()
}

const GIB: u64 = 1024 * 1024 * 1024;
static RESERVED: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

fn setting(name: &str, default: u64) -> Result<u64> {
    let value = match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .with_context(|| format!("Invalid {name}"))?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(value > 0, "{name} must be positive");
    Ok(value)
}

pub fn validate_config() -> Result<()> {
    setting("MAX_STORAGE_BYTES", 30 * GIB)?;
    setting("MIN_FREE_DISK_BYTES", 5 * GIB)?;
    anyhow::ensure!(
        setting("UPLOAD_TIMEOUT_SECONDS", 300)? <= 3600,
        "UPLOAD_TIMEOUT_SECONDS must be between 1 and 3600"
    );
    Ok(())
}

pub fn directory_size(root: &Path) -> Result<u64> {
    let mut size = 0u64;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error)
                if error
                    .io_error()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if entry.file_type().is_file() {
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error)
                    if error
                        .io_error()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            size = size
                .checked_add(metadata.len())
                .context("Storage size overflow")?;
        }
    }
    Ok(size)
}

struct Reservation(u64);
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut reserved = RESERVED.lock().unwrap_or_else(|e| e.into_inner());
        *reserved -= self.0;
    }
}

fn capacity_allows(used: u64, free: u64, reserved: u64, maximum: u64, minimum: u64) -> bool {
    used.checked_add(reserved).is_some_and(|n| n <= maximum)
        && free.checked_sub(reserved).is_some_and(|n| n >= minimum)
}

fn reserve(root: &Path, bytes: u64) -> Result<Reservation> {
    fs::create_dir_all(root)?;
    let mut reserved = RESERVED.lock().unwrap_or_else(|e| e.into_inner());
    let total = reserved
        .checked_add(bytes)
        .context("Reservation overflow")?;
    anyhow::ensure!(
        capacity_allows(
            directory_size(root)?,
            fs2::available_space(root)?,
            total,
            setting("MAX_STORAGE_BYTES", 30 * GIB)?,
            setting("MIN_FREE_DISK_BYTES", 5 * GIB)?
        ),
        "Insufficient storage capacity"
    );
    *reserved = total;
    Ok(Reservation(bytes))
}

pub fn check_capacity() -> Result<()> {
    let root = Path::new("storage");
    anyhow::ensure!(
        directory_size(root)? <= setting("MAX_STORAGE_BYTES", 30 * GIB)?
            && fs2::available_space(root)? >= setting("MIN_FREE_DISK_BYTES", 5 * GIB)?,
        "Storage capacity exhausted"
    );
    Ok(())
}

pub struct Staging {
    directory: tempfile::TempDir,
    uploaded_at: String,
    _reservation: Reservation,
}
impl Staging {
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
}

fn staging(bytes: u64) -> Result<Staging> {
    let reservation = reserve(Path::new("storage"), bytes)?;
    fs::create_dir_all("storage/staging")?;
    Ok(Staging {
        directory: tempfile::tempdir_in("storage/staging")?,
        uploaded_at: crate::audit::now(),
        _reservation: reservation,
    })
}

pub async fn prepare(bytes: u64) -> Result<Staging> {
    tokio::task::spawn_blocking(move || staging(bytes)).await?
}

pub fn check_free_space(root: &Path) -> Result<()> {
    anyhow::ensure!(
        fs2::available_space(root)? >= setting("MIN_FREE_DISK_BYTES", 5 * GIB)?,
        "Free disk reserve exhausted"
    );
    Ok(())
}

// Publish only completed work. The renamed job folder contains originals and results.
pub fn publish(work: &Staging, output: &str, uploader_ip: std::net::IpAddr) -> Result<String> {
    check_capacity()?;
    let date = chrono::DateTime::parse_from_rfc3339(&work.uploaded_at)?
        .format("%d%m%Y")
        .to_string();
    let (id, kind) = output.rsplit_once('.').context("Invalid output filename")?;
    let directory = Path::new("storage/active").join(&date);
    fs::create_dir_all(&directory)?;
    let destination = directory.join(id);
    let stored_size = fs::metadata(work.path().join(output))?.len();
    let original_size = original_size(work.path(), output)?;
    crate::audit::database()?.publish(
        &crate::audit::FileRecord {
            id,
            file_type: kind,
            uploaded_at: &work.uploaded_at,
            uploader_ip: Some(uploader_ip),
            original_size,
            stored_size,
        },
        || {
            fs::rename(work.path(), &destination)?;
            Ok(())
        },
        || {
            if let Err(error) = fs::rename(&destination, work.path()) {
                tracing::error!(%error,"Cannot roll back publication");
            }
        },
    )?;
    Ok(format!("/api/files/{date}/{output}"))
}

fn original_size(directory: &Path, output: &str) -> Result<u64> {
    fs::read_dir(directory)?.try_fold(0u64, |size, entry| -> Result<u64> {
        let entry = entry?;
        Ok(if entry.file_name() != output {
            size + entry.metadata()?.len()
        } else {
            size
        })
    })
}

// YouTube inputs are transient download/merge intermediates, unlike browser uploads.
pub async fn publish_youtube(
    work: Staging,
    output: String,
    uploader_ip: std::net::IpAddr,
) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        for entry in fs::read_dir(work.path())? {
            let entry = entry?;
            if entry.file_name() == output.as_str() {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs::remove_dir(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        publish(&work, &output, uploader_ip)
    })
    .await?
}

fn valid_filename(filename: &str) -> bool {
    filename.rsplit_once('.').is_some_and(|(id, ext)| {
        uuid::Uuid::parse_str(id).is_ok()
            && matches!(
                ext,
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "ico"
                    | "bmp"
                    | "tif"
                    | "tiff"
                    | "mp3"
                    | "mp4"
            )
    })
}

pub async fn download_handler(
    RoutePath((date, filename)): RoutePath<(String, String)>,
    axum::Extension(crate::rate_limit::ClientIp(viewer_ip)): axum::Extension<
        crate::rate_limit::ClientIp,
    >,
) -> Result<Response, StatusCode> {
    if !valid_date(&date) || !valid_filename(&filename) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let id = filename.rsplit_once('.').unwrap().0;
    tracing::info!("File id: {id} download requested");
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
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => "image/x-icon",
    };
    crate::audit::access(id.to_owned(), viewer_ip)
        .await
        .map_err(|error| {
            tracing::error!(%error, "Cannot audit file access");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
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
            let Ok(_permit) = crate::resources::acquire() else {
                tracing::info!("Storage maintenance deferred while media jobs are busy");
                continue;
            };
            match tokio::task::spawn_blocking(|| {
                let audit = crate::audit::database()?;
                audit.import_storage(Path::new("storage"))?;
                audit.reconcile(Path::new("storage"))?;
                archive_expired(Path::new("storage"), SystemTime::now(), Some(audit))
            })
            .await
            {
                Ok(Ok(())) => {}
                result => {
                    tracing::error!(
                        ?result,
                        "Storage maintenance failed; will retry in one hour"
                    )
                }
            }
        }
    });
}

fn copy_archive_file(
    root: &Path,
    source: &mut impl std::io::Read,
    destination: &mut impl std::io::Write,
) -> Result<()> {
    let mut buffer = [0u8; 128 * 1024];
    let mut checked = std::time::Instant::now();
    check_free_space(root)?;
    loop {
        let size = source.read(&mut buffer)?;
        if size == 0 {
            return Ok(());
        }
        if checked.elapsed() >= Duration::from_millis(250) {
            check_free_space(root)?;
            checked = std::time::Instant::now();
        }
        destination.write_all(&buffer[..size])?;
    }
}

fn file_id(path: &Path) -> Option<String> {
    let name = path.file_stem()?.to_str()?;
    if let Ok(id) = uuid::Uuid::parse_str(name) {
        return Some(id.to_string());
    }
    uuid::Uuid::parse_str(path.parent()?.file_name()?.to_str()?)
        .ok()
        .map(|id| id.to_string())
}

fn archive_expired(
    root: &Path,
    now: SystemTime,
    audit: Option<&crate::audit::Audit>,
) -> Result<()> {
    delete_expired_archives(root, now, audit)?;
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
            if let Some(audit) = audit
                && let Some(id) = file_id(entry.path())
                && audit.archived_copy(root, &id, entry.path())?
            {
                // Retry removal only when this exact source entry is in a finalized ZIP.
                fs::remove_file(entry.path())?;
                continue;
            }
            if area == "active" {
                let parent = entry.path().parent().context("Missing job folder")?;
                if fs::read_dir(parent)?.try_fold(false, |recent, file| -> Result<bool> {
                    Ok(recent
                        || now
                            .duration_since(file?.metadata()?.modified()?)
                            .unwrap_or_default()
                            < RETENTION)
                })? {
                    continue;
                }
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
        // Reserve space for an incompressible ZIP plus headers before duplicating originals.
        let bytes = files
            .iter()
            .try_fold(1024 * 1024u64, |sum, path| -> Result<u64> {
                sum.checked_add(fs::metadata(path)?.len().saturating_add(4096))
                    .context("Archive size overflow")
            })?;
        let _reservation = reserve(root, bytes.saturating_add(bytes / 100))?;
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
                copy_archive_file(root, &mut fs::File::open(path)?, &mut writer)?;
            }
            if let Some(audit) = audit {
                let mut timestamps = audit.upload_timestamps()?;
                timestamps.retain(|id, _| {
                    files
                        .iter()
                        .any(|path| path.file_stem().and_then(|s| s.to_str()) == Some(id.as_str()))
                });
                writer.start_file(".audit-upload-times.json", options)?;
                serde_json::to_writer(&mut writer, &timestamps)?;
            }
            writer.finish()?;
        }
        check_free_space(root)?;
        temporary.as_file().sync_all()?;
        // Verify every entry's CRC before removing any source files.
        {
            let mut archive = zip::ZipArchive::new(temporary.reopen()?)?;
            for index in 0..archive.len() {
                std::io::copy(&mut archive.by_index(index)?, &mut std::io::sink())?;
            }
        }
        let archive_path = destination.join(format!("{}.zip", uuid::Uuid::new_v4()));
        temporary.persist(&archive_path)?;
        if let Some(audit) = audit {
            let ids = files
                .iter()
                .filter_map(|path| file_id(path))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            audit.archived(
                &ids,
                &archive_path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            )?;
        }
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

fn delete_expired_archives(
    root: &Path,
    now: SystemTime,
    audit: Option<&crate::audit::Audit>,
) -> Result<()> {
    let archives = root.join("archives");
    if !archives.exists() {
        return Ok(());
    }
    for day in fs::read_dir(&archives)? {
        let day = day?;
        if !day.file_type()?.is_dir() || !valid_date(&day.file_name().to_string_lossy()) {
            continue;
        }
        for entry in fs::read_dir(day.path())? {
            let entry = entry?;
            let path = entry.path();
            // Only finalized ZIP files inside date folders are eligible.
            if !entry.file_type()?.is_file()
                || path.extension().and_then(|ext| ext.to_str()) != Some("zip")
            {
                continue;
            }
            if now
                .duration_since(entry.metadata()?.modified()?)
                .unwrap_or_default()
                >= ARCHIVE_RETENTION
            {
                if let Some(audit) = audit {
                    audit.delete_archive(
                        root,
                        &path
                            .strip_prefix(root)?
                            .to_string_lossy()
                            .replace('\\', "/"),
                        || {
                            fs::remove_file(&path)?;
                            Ok(())
                        },
                    )?;
                } else {
                    fs::remove_file(&path)?;
                }
                tracing::info!(archive = %path.display(), "Expired archive deleted");
            }
        }
        // Keep non-empty folders, including folders with unfinished work.
        if fs::read_dir(day.path())?.next().is_none() {
            fs::remove_dir(day.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn publication_counts_only_browser_originals() -> Result<()> {
        // No upload input exists for a YouTube-only result.
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("result.mp4"), b"result")?;
        let size = original_size(root.path(), "result.mp4")?;
        assert_eq!(size, 0);
        fs::write(root.path().join("source.mp4"), b"browser input")?;
        assert_eq!(original_size(root.path(), "result.mp4")?, 13);
        Ok(())
    }

    #[test]
    fn audit_follows_archive_and_deletion_lifecycle() -> Result<()> {
        let root = tempfile::tempdir()?;
        let database_path = root.path().join("audit.sqlite3");
        let audit = crate::audit::Audit::open(&database_path)?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = SystemTime::now();
        let uploaded_at = DateTime::<Utc>::from(now - RETENTION - Duration::from_secs(1));
        let job = root
            .path()
            .join("active")
            .join(uploaded_at.format("%d%m%Y").to_string())
            .join(&id);
        fs::create_dir_all(&job)?;
        for name in ["source.png".to_owned(), format!("{id}.png")] {
            let file = job.join(name);
            fs::write(&file, b"bytes")?;
            fs::File::options().write(true).open(file)?.set_times(
                fs::FileTimes::new().set_modified(now - RETENTION - Duration::from_secs(1)),
            )?;
        }
        audit.publish(
            &crate::audit::FileRecord {
                id: &id,
                file_type: "png",
                uploaded_at: &uploaded_at.to_rfc3339(),
                uploader_ip: Some("192.0.2.1".parse()?),
                original_size: 5,
                stored_size: 5,
            },
            || Ok(()),
            || {},
        )?;
        audit.access(&id, "198.51.100.1".parse()?)?;
        archive_expired(root.path(), now, Some(&audit))?;
        let connection = rusqlite::Connection::open(&database_path)?;
        let archive: String =
            connection.query_row("SELECT archive_path FROM files WHERE id=?1", [&id], |r| {
                r.get(0)
            })?;
        assert!(!job.exists());
        assert_eq!(
            connection.query_row("SELECT archived FROM files WHERE id=?1", [&id], |r| r
                .get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM ip_uploads", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        fs::File::options()
            .write(true)
            .open(root.path().join(&archive))?
            .set_times(fs::FileTimes::new().set_modified(now - ARCHIVE_RETENTION))?;
        delete_expired_archives(root.path(), now, Some(&audit))?;
        assert!(!root.path().join(archive).exists());
        assert_eq!(
            connection.query_row("SELECT count(*) FROM ip_uploads", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM file_access", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        assert_eq!(
            connection.query_row("SELECT count(*) FROM files WHERE id=?1", [&id], |r| r
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    }

    #[test]
    fn capacity_includes_reservations_and_preserves_free_space() {
        assert!(capacity_allows(20, 15, 5, 30, 5));
        assert!(!capacity_allows(26, 15, 5, 30, 5));
        assert!(!capacity_allows(20, 9, 5, 30, 5));
        assert!(!capacity_allows(u64::MAX, u64::MAX, 1, u64::MAX, 1));
        assert!(!capacity_allows(0, 1, 2, 30, 1));
    }

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
        let recent_day = root.path().join("active/01102026/recent-job");
        fs::create_dir_all(&recent_day)?;
        let recent = recent_day.join("recent.png");
        fs::write(&old, b"original bytes")?;
        fs::write(&recent, b"recent bytes")?;
        let now = SystemTime::now();
        fs::File::options()
            .write(true)
            .open(&old)?
            .set_times(fs::FileTimes::new().set_modified(now - RETENTION))?;
        archive_expired(root.path(), now, None)?;
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
        archive_expired(root.path(), now, None)?;
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
        assert!(archive_expired(root.path(), now, None).is_err());
        assert_eq!(fs::read(source)?, b"audio bytes");
        Ok(())
    }

    #[test]
    fn deletes_archives_at_thirty_days_and_preserves_other_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let day = root.path().join("archives/01102026");
        fs::create_dir_all(&day)?;
        let expired = day.join("expired.zip");
        let recent = day.join("recent.zip");
        let temporary = day.join("unfinished.tmp");
        let now = SystemTime::now();
        for path in [&expired, &recent, &temporary] {
            fs::write(path, b"test data")?;
        }
        for path in [&expired, &temporary] {
            fs::File::options()
                .write(true)
                .open(path)?
                .set_times(fs::FileTimes::new().set_modified(now - ARCHIVE_RETENTION))?;
        }
        fs::File::options().write(true).open(&recent)?.set_times(
            fs::FileTimes::new().set_modified(now - ARCHIVE_RETENTION + Duration::from_secs(1)),
        )?;
        delete_expired_archives(root.path(), now, None)?;
        assert!(!expired.exists());
        assert!(recent.exists());
        assert!(temporary.exists());
        delete_expired_archives(root.path(), now + Duration::from_secs(1), None)?;
        assert!(!recent.exists());
        assert!(temporary.exists());
        Ok(())
    }
}
