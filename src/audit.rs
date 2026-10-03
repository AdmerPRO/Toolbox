use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    net::IpAddr,
    path::Path,
    sync::{Mutex, OnceLock},
    time::Duration,
};

static DATABASE: OnceLock<Audit> = OnceLock::new();
pub const POLICY_VERSION: &str = "2026-10-03";

pub struct Audit {
    connection: Mutex<Connection>,
}
pub struct FileRecord<'a> {
    pub id: &'a str,
    pub file_type: &'a str,
    pub uploaded_at: &'a str,
    pub uploader_ip: Option<IpAddr>,
    pub original_size: u64,
    pub stored_size: u64,
}

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
pub fn initialize() -> Result<()> {
    std::fs::create_dir_all("storage")?;
    let audit = Audit::open(Path::new("storage/audit.sqlite3"))?;
    audit.import_storage(Path::new("storage"))?;
    audit.reconcile(Path::new("storage"))?;
    DATABASE
        .set(audit)
        .map_err(|_| anyhow::anyhow!("Audit database already initialized"))?;
    Ok(())
}
pub fn database() -> Result<&'static Audit> {
    DATABASE.get().context("Audit database is not initialized")
}

impl Audit {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON;
            CREATE TABLE IF NOT EXISTS files (
                id TEXT PRIMARY KEY, file_type TEXT NOT NULL, uploaded_at TEXT NOT NULL,
                uploader_ip TEXT, original_size INTEGER NOT NULL CHECK(original_size >= 0),
                stored_size INTEGER NOT NULL CHECK(stored_size >= 0), archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
                archived_at TEXT, deleted_at TEXT, open_count INTEGER NOT NULL DEFAULT 0 CHECK(open_count >= 0),
                archive_path TEXT, policy_version TEXT
            );
            CREATE TABLE IF NOT EXISTS file_access (
                file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                viewer_ip TEXT NOT NULL, first_access_at TEXT NOT NULL, last_access_at TEXT NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 1 CHECK(access_count > 0), PRIMARY KEY(file_id, viewer_ip)
            );
            CREATE TABLE IF NOT EXISTS ip_uploads (
                ip TEXT NOT NULL, file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                uploaded_at TEXT NOT NULL, PRIMARY KEY(ip, file_id), UNIQUE(file_id)
            );
            CREATE INDEX IF NOT EXISTS files_archive ON files(archive_path);
            CREATE INDEX IF NOT EXISTS files_uploader ON files(uploader_ip);
            CREATE INDEX IF NOT EXISTS file_access_viewer ON file_access(viewer_ip);")?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn import_storage(&self, root: &Path) -> Result<()> {
        for area in ["active", "ytmp3", "ytmp4"] {
            let directory = root.join(area);
            if !directory.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(directory).follow_links(false) {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let Some((id, kind)) = entry.file_name().to_str().and_then(|s| s.rsplit_once('.'))
                else {
                    continue;
                };
                if uuid::Uuid::parse_str(id).is_err()
                    || !matches!(
                        kind,
                        "png"
                            | "jpg"
                            | "jpeg"
                            | "webp"
                            | "ico"
                            | "bmp"
                            | "tif"
                            | "tiff"
                            | "mp3"
                            | "mp4"
                    )
                {
                    continue;
                }
                let metadata = entry.metadata()?;
                let mut timestamp = chrono::DateTime::<Utc>::from(metadata.modified()?);
                if area == "active"
                    && let Some(day) = entry
                        .path()
                        .strip_prefix(root.join("active"))?
                        .components()
                        .next()
                        .and_then(|d| d.as_os_str().to_str())
                    && let Ok(day) = chrono::NaiveDate::parse_from_str(day, "%d%m%Y")
                {
                    timestamp = day.and_time(timestamp.time()).and_utc();
                }
                let uploaded_at = timestamp.to_rfc3339_opts(SecondsFormat::Millis, true);
                let original_size =
                    std::fs::read_dir(entry.path().parent().context("No file parent")?)?
                        .filter_map(|e| e.ok())
                        .filter(|e| e.file_name().to_string_lossy().starts_with("source."))
                        .try_fold(0u64, |sum, e| {
                            Ok::<_, std::io::Error>(sum + e.metadata()?.len())
                        })?;
                self.import(
                    &FileRecord {
                        id,
                        file_type: kind,
                        uploaded_at: &uploaded_at,
                        uploader_ip: None,
                        original_size,
                        stored_size: metadata.len(),
                    },
                    None,
                )?;
            }
        }
        let archives = root.join("archives");
        if archives.exists() {
            for entry in walkdir::WalkDir::new(archives).follow_links(false) {
                let entry = entry?;
                if !entry.file_type().is_file()
                    || entry.path().extension().and_then(|e| e.to_str()) != Some("zip")
                {
                    continue;
                }
                let archive_name = entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                let uploaded_at = chrono::DateTime::<Utc>::from(entry.metadata()?.modified()?)
                    .to_rfc3339_opts(SecondsFormat::Millis, true);
                let mut archive = zip::ZipArchive::new(std::fs::File::open(entry.path())?)?;
                for index in 0..archive.len() {
                    let file = archive.by_index(index)?;
                    let name = Path::new(file.name());
                    let Some((id, kind)) = name
                        .file_name()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.rsplit_once('.'))
                    else {
                        continue;
                    };
                    if uuid::Uuid::parse_str(id).is_err()
                        || !matches!(
                            kind,
                            "png"
                                | "jpg"
                                | "jpeg"
                                | "webp"
                                | "ico"
                                | "bmp"
                                | "tif"
                                | "tiff"
                                | "mp3"
                                | "mp4"
                        )
                    {
                        continue;
                    }
                    self.import(
                        &FileRecord {
                            id,
                            file_type: kind,
                            uploaded_at: &uploaded_at,
                            uploader_ip: None,
                            original_size: 0,
                            stored_size: file.size(),
                        },
                        Some(&archive_name),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn import(&self, record: &FileRecord<'_>, archive: Option<&str>) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        connection.execute("INSERT OR IGNORE INTO files (id,file_type,uploaded_at,original_size,stored_size,archived,archived_at,archive_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![record.id,record.file_type,record.uploaded_at,i64::try_from(record.original_size)?,i64::try_from(record.stored_size)?,i32::from(archive.is_some()),archive.map(|_|record.uploaded_at),archive])?;
        // Recover an archive finalized just before the previous process crashed.
        if let Some(archive) = archive {
            connection.execute("UPDATE files SET archived=1,archived_at=COALESCE(archived_at,?2),archive_path=?3 WHERE id=?1 AND deleted_at IS NULL AND archive_path IS NULL",params![record.id,record.uploaded_at,archive])?;
        }
        Ok(())
    }

    pub fn publish(
        &self,
        record: &FileRecord<'_>,
        move_files: impl FnOnce() -> Result<()>,
        undo_move: impl FnOnce(),
    ) -> Result<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        let transaction = connection.transaction()?;
        let ip = record.uploader_ip.map(|ip| ip.to_canonical().to_string());
        transaction.execute("INSERT INTO files (id,file_type,uploaded_at,uploader_ip,original_size,stored_size,policy_version) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![record.id,record.file_type,record.uploaded_at,ip,i64::try_from(record.original_size)?,i64::try_from(record.stored_size)?,ip.as_ref().map(|_| POLICY_VERSION)])?;
        if let Some(ip) = ip {
            transaction.execute(
                "INSERT INTO ip_uploads(ip,file_id,uploaded_at) VALUES (?1,?2,?3)",
                params![ip, record.id, record.uploaded_at],
            )?;
        }
        move_files()?;
        if let Err(error) = transaction.commit() {
            undo_move();
            return Err(error.into());
        }
        Ok(())
    }

    pub fn access(&self, id: &str, ip: IpAddr) -> Result<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE files SET open_count=open_count+1 WHERE id=?1 AND deleted_at IS NULL",
            [id],
        )?;
        anyhow::ensure!(changed == 1, "File has no live audit record");
        let timestamp = now();
        transaction.execute("INSERT INTO file_access(file_id,viewer_ip,first_access_at,last_access_at,access_count) VALUES (?1,?2,?3,?3,1)
            ON CONFLICT(file_id,viewer_ip) DO UPDATE SET last_access_at=excluded.last_access_at,access_count=access_count+1",
            params![id,ip.to_canonical().to_string(),timestamp])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn archived(&self, ids: &[String], archive: &str) -> Result<()> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        let transaction = connection.transaction()?;
        let timestamp = now();
        for id in ids {
            transaction.execute("UPDATE files SET archived=1,archived_at=?2,archive_path=?3 WHERE id=?1 AND deleted_at IS NULL", params![id,timestamp,archive])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn archived_copy(&self, root: &Path, id: &str, source: &Path) -> Result<bool> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        let archive: Option<String> = connection
            .query_row(
                "SELECT archive_path FROM files WHERE id=?1 AND deleted_at IS NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let Some(path) = archive else {
            return Ok(false);
        };
        let path = root.join(path);
        if !path.try_exists()? {
            return Ok(false);
        }
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
        Ok(archive
            .by_name(
                &source
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
            .is_ok())
    }

    pub fn delete_archive(
        &self,
        root: &Path,
        archive: &str,
        delete: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let inventory = archive_inventory(root)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        let records = {
            let mut statement = connection.prepare("SELECT id,file_type,uploaded_at FROM files WHERE archive_path=?1 AND deleted_at IS NULL")?;
            statement
                .query_map([archive], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let transaction = connection.transaction()?;
        for (id, kind, timestamp) in records {
            if let Some((path, _)) = inventory
                .iter()
                .find(|(path, ids)| path.as_str() != archive && ids.contains(&id))
            {
                transaction.execute(
                    "UPDATE files SET archive_path=?2 WHERE id=?1",
                    params![id, path],
                )?;
            } else if active_exists(root, &id, &kind, &timestamp)? {
                transaction.execute("UPDATE files SET archive_path=NULL WHERE id=?1", [&id])?;
            } else {
                transaction.execute("DELETE FROM file_access WHERE file_id=?1", [&id])?;
                transaction.execute("DELETE FROM ip_uploads WHERE file_id=?1", [&id])?;
                transaction.execute(
                    "UPDATE files SET uploader_ip=NULL,deleted_at=?2 WHERE id=?1",
                    params![id, now()],
                )?;
            }
        }
        delete()?;
        transaction.commit()?;
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    // Repair cleanup after a crash or an operator deleting files directly on disk.
    pub fn reconcile(&self, root: &Path) -> Result<()> {
        let inventory = archive_inventory(root)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        let records = {
            let mut statement = connection.prepare(
                "SELECT id,file_type,uploaded_at,archive_path FROM files WHERE deleted_at IS NULL",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let transaction = connection.transaction()?;
        for (id, kind, timestamp, archive) in records {
            let exists = if let Some(archive) = archive {
                // An archive may have committed before active originals were removed.
                inventory.get(&archive).is_some_and(|ids| ids.contains(&id))
                    || active_exists(root, &id, &kind, &timestamp)?
            } else {
                active_exists(root, &id, &kind, &timestamp)?
            };
            let other_archive = inventory
                .iter()
                .find(|(_, ids)| ids.contains(&id))
                .map(|(path, _)| path);
            if let Some(path) = other_archive {
                transaction.execute(
                    "UPDATE files SET archive_path=?2,archived=1 WHERE id=?1",
                    params![id, path],
                )?;
            }
            if !exists && other_archive.is_none() {
                transaction.execute("DELETE FROM file_access WHERE file_id=?1", [&id])?;
                transaction.execute("DELETE FROM ip_uploads WHERE file_id=?1", [&id])?;
                transaction.execute(
                    "UPDATE files SET uploader_ip=NULL,deleted_at=?2 WHERE id=?1",
                    params![id, now()],
                )?;
            }
        }
        transaction.commit()?;
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn archive_inventory(
    root: &Path,
) -> Result<std::collections::BTreeMap<String, std::collections::BTreeSet<String>>> {
    let mut inventory = std::collections::BTreeMap::new();
    let directory = root.join("archives");
    if !directory.exists() {
        return Ok(inventory);
    }
    for entry in walkdir::WalkDir::new(directory).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|s| s.to_str()) != Some("zip")
        {
            continue;
        }
        let path = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let mut ids = std::collections::BTreeSet::new();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(entry.path())?)?;
        for index in 0..archive.len() {
            let file = archive.by_index(index)?;
            let path = Path::new(file.name());
            for candidate in [path.file_stem(), path.parent().and_then(|p| p.file_name())]
                .into_iter()
                .flatten()
            {
                if let Some(id) = candidate
                    .to_str()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                {
                    ids.insert(id.to_string());
                }
            }
        }
        inventory.insert(path, ids);
    }
    Ok(inventory)
}

fn active_exists(root: &Path, id: &str, kind: &str, timestamp: &str) -> Result<bool> {
    let date = chrono::DateTime::parse_from_rfc3339(timestamp)?
        .format("%d%m%Y")
        .to_string();
    let job = root.join("active").join(date).join(id);
    let retained_job = if job.try_exists()? {
        std::fs::read_dir(job)?.next().transpose()?.is_some()
    } else {
        false
    };
    Ok(retained_job
        || root
            .join(if kind == "mp3" { "ytmp3" } else { "ytmp4" })
            .join(format!("{id}.{kind}"))
            .try_exists()?)
}

pub async fn access(id: String, ip: IpAddr) -> Result<()> {
    tokio::task::spawn_blocking(move || database()?.access(&id, ip)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(audit: &Audit, id: &str) -> Result<()> {
        audit.publish(
            &FileRecord {
                id,
                file_type: "png",
                uploaded_at: "2026-10-03T10:41:22.123Z",
                uploader_ip: Some("192.0.2.1".parse()?),
                original_size: 184,
                stored_size: 121,
            },
            || Ok(()),
            || {},
        )
    }
    #[test]
    fn access_counts_and_atomic_ip_cleanup() -> Result<()> {
        let root = tempfile::tempdir()?;
        let audit = Audit::open(&root.path().join("audit.sqlite3"))?;
        add(&audit, "file1")?;
        add(&audit, "file2")?;
        audit.access("file1", "198.51.100.1".parse()?)?;
        audit.access("file1", "198.51.100.1".parse()?)?;
        audit.access("file1", "198.51.100.2".parse()?)?;
        {
            let connection = audit.connection.lock().unwrap();
            assert_eq!(
                connection.query_row("SELECT open_count FROM files WHERE id='file1'", [], |r| r
                    .get::<_, i64>(0))?,
                3
            );
            assert_eq!(
                connection.query_row(
                    "SELECT access_count FROM file_access WHERE viewer_ip='198.51.100.1'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                2
            );
            assert_eq!(
                connection.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                3
            );
        }
        audit.archived(&["file1".into()], "archives/test.zip")?;
        assert!(
            audit
                .delete_archive(root.path(), "archives/test.zip", || anyhow::bail!(
                    "Cannot delete ZIP"
                ))
                .is_err()
        );
        audit.delete_archive(root.path(), "archives/test.zip", || Ok(()))?;
        {
            let connection = audit.connection.lock().unwrap();
            assert_eq!(
                connection.query_row(
                    "SELECT count(*) FROM file_access WHERE file_id='file1'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            assert_eq!(
                connection.query_row(
                    "SELECT count(*) FROM ip_uploads WHERE ip='192.0.2.1'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert!(
                connection
                    .query_row("SELECT uploader_ip FROM files WHERE id='file1'", [], |r| {
                        r.get::<_, Option<String>>(0)
                    })?
                    .is_none()
            );
            assert!(
                connection
                    .query_row("SELECT deleted_at FROM files WHERE id='file1'", [], |r| {
                        r.get::<_, Option<String>>(0)
                    })?
                    .is_some()
            );
        }
        audit.archived(&["file2".into()], "archives/other.zip")?;
        audit.delete_archive(root.path(), "archives/other.zip", || Ok(()))?;
        assert_eq!(
            audit.connection.lock().unwrap().query_row(
                "SELECT count(*) FROM ip_uploads",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        assert!(audit.access("file1", "198.51.100.1".parse()?).is_err());
        Ok(())
    }
    #[test]
    fn retains_ips_until_the_last_archive_copy_is_removed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let audit = Audit::open(&root.path().join("audit.sqlite3"))?;
        let id = uuid::Uuid::new_v4().to_string();
        add(&audit, &id)?;
        let archives = root.path().join("archives/03102026");
        std::fs::create_dir_all(&archives)?;
        for name in ["first.zip", "second.zip"] {
            let mut writer = zip::ZipWriter::new(std::fs::File::create(archives.join(name))?);
            writer.start_file(
                format!("active/03102026/{id}/{id}.png"),
                zip::write::SimpleFileOptions::default(),
            )?;
            std::io::Write::write_all(&mut writer, b"image")?;
            writer.finish()?;
        }
        audit.archived(std::slice::from_ref(&id), "archives/03102026/first.zip")?;
        audit.delete_archive(root.path(), "archives/03102026/first.zip", || {
            std::fs::remove_file(archives.join("first.zip"))?;
            Ok(())
        })?;
        assert_eq!(
            audit.connection.lock().unwrap().query_row(
                "SELECT count(*) FROM ip_uploads",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        // Simulate an operator rebuilding the remaining shared ZIP without this job.
        let writer = zip::ZipWriter::new(std::fs::File::create(archives.join("second.zip"))?);
        writer.finish()?;
        audit.reconcile(root.path())?;
        assert_eq!(
            audit.connection.lock().unwrap().query_row(
                "SELECT count(*) FROM ip_uploads",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }

    #[test]
    fn publication_failure_leaves_no_upload_records() -> Result<()> {
        let root = tempfile::tempdir()?;
        let audit = Audit::open(&root.path().join("audit.sqlite3"))?;
        let result = audit.publish(
            &FileRecord {
                id: "test",
                file_type: "mp4",
                uploaded_at: "2026-10-03T00:00:00Z",
                uploader_ip: Some("192.0.2.1".parse()?),
                original_size: 1,
                stored_size: 1,
            },
            || anyhow::bail!("Rename failed"),
            || {},
        );
        assert!(result.is_err());
        assert_eq!(
            audit
                .connection
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))?,
            0
        );
        Ok(())
    }
    #[test]
    fn reconciliation_keeps_originals_and_cleans_deleted_jobs() -> Result<()> {
        let root = tempfile::tempdir()?;
        let audit = Audit::open(&root.path().join("audit.sqlite3"))?;
        let id = uuid::Uuid::new_v4().to_string();
        add(&audit, &id)?;
        let job = root.path().join("active/03102026").join(&id);
        std::fs::create_dir_all(&job)?;
        std::fs::write(job.join("source.png"), b"source")?;
        audit.reconcile(root.path())?;
        assert!(
            audit
                .connection
                .lock()
                .unwrap()
                .query_row("SELECT uploader_ip FROM files WHERE id=?1", [&id], |r| {
                    r.get::<_, Option<String>>(0)
                })?
                .is_some()
        );
        std::fs::remove_file(job.join("source.png"))?;
        audit.reconcile(root.path())?;
        assert_eq!(
            audit.connection.lock().unwrap().query_row(
                "SELECT count(*) FROM ip_uploads",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }
}
