use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Read,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, Weak},
    time::SystemTime,
};

// Weak entries disappear once the last worker finishes; waiting workers share one lock.
static LOCKS: LazyLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub async fn lock(key: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut locks = LOCKS.lock().unwrap_or_else(|error| error.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(key).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                locks.insert(key.to_owned(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    lock.lock_owned().await
}

pub fn key(operation: &str, source: &str, options: &str) -> String {
    // Length-prefixed JSON fields keep operation, source and settings unambiguous.
    let bytes =
        serde_json::to_vec(&("v1", operation, source, options)).expect("String serialization");
    format!("{:x}", Sha256::digest(bytes))
}

pub fn file_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn result_path(root: &Path, url: &str) -> Option<(String, PathBuf)> {
    let tail = url.strip_prefix("/api/files/")?;
    let (date, filename) = tail.split_once('/')?;
    if !crate::storage::valid_date(date) || !crate::storage::valid_filename(filename) {
        return None;
    }
    let id = filename.rsplit_once('.')?.0;
    Some((
        id.to_owned(),
        root.join("active").join(date).join(id).join(filename),
    ))
}

impl crate::audit::Audit {
    pub fn cached_result(&self, key: &str, root: &Path, ip: IpAddr) -> Result<Option<String>> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM result_cache WHERE file_id IN (SELECT id FROM files WHERE archived=1)",
            [],
        )?;
        let entry: Option<(String, String)> = transaction.query_row(
            "SELECT c.file_id,c.download_url FROM result_cache c JOIN files f ON f.id=c.file_id WHERE c.cache_key=?1 AND f.archived=0 AND f.deleted_at IS NULL",
            [key], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let Some((id, url)) = entry else {
            transaction.commit()?;
            return Ok(None);
        };
        let fresh = result_path(root, &url)
            .filter(|(parsed_id, _)| parsed_id == &id)
            .and_then(|(_, path)| std::fs::metadata(path).ok())
            .filter(|metadata| metadata.is_file() && metadata.len() > 0)
            .and_then(|metadata| metadata.modified().ok())
            .is_some_and(|modified| {
                SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default()
                    < crate::storage::RETENTION
            });
        if !fresh {
            transaction.execute("DELETE FROM result_cache WHERE cache_key=?1", [key])?;
            transaction.commit()?;
            tracing::info!(cache_key = key, "Result cache entry expired or missing");
            return Ok(None);
        }
        let now = crate::audit::now();
        transaction.execute(
            "INSERT INTO cache_submissions(file_id,ip,first_requested_at,last_requested_at) VALUES (?1,?2,?3,?3) ON CONFLICT(file_id,ip) DO UPDATE SET last_requested_at=excluded.last_requested_at,request_count=request_count+1",
            params![id, ip.to_canonical().to_string(), now],
        )?;
        transaction.commit()?;
        Ok(Some(url))
    }

    pub fn cache_result(&self, key: &str, url: &str) -> Result<()> {
        let (id, _) = result_path(Path::new("storage"), url).context("Invalid cache result URL")?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Audit lock poisoned"))?;
        connection.execute("INSERT INTO result_cache(cache_key,file_id,download_url) VALUES (?1,?2,?3) ON CONFLICT(cache_key) DO UPDATE SET file_id=excluded.file_id,download_url=excluded.download_url", params![key,id,url])?;
        Ok(())
    }
}

pub async fn lookup(key: &str, ip: IpAddr) -> Result<Option<String>> {
    let key = key.to_owned();
    tokio::task::spawn_blocking(move || {
        let result = crate::audit::database()?.cached_result(&key, Path::new("storage"), ip)?;
        tracing::info!(cache_key = %key, hit = result.is_some(), "Result cache lookup");
        Ok(result)
    })
    .await?
}

pub fn store(key: &str, url: &str) {
    // A cache write failure must not discard an already published result.
    if let Err(error) = crate::audit::database().and_then(|db| db.cache_result(key, url)) {
        tracing::warn!(%error, "Could not store result cache entry");
    } else {
        tracing::info!(cache_key = key, "Result cached");
    }
}

pub async fn store_async(key: String, url: String) {
    if let Err(error) = tokio::task::spawn_blocking(move || store(&key, &url)).await {
        tracing::warn!(%error, "Result cache worker failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (
        tempfile::TempDir,
        crate::audit::Audit,
        String,
        String,
        PathBuf,
    ) {
        let root = tempfile::tempdir().unwrap();
        let db = crate::audit::Audit::open(&root.path().join("audit.sqlite3")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let filename = format!("{id}.mp4");
        let date = chrono::Utc::now().format("%d%m%Y").to_string();
        let directory = root.path().join("active").join(&date).join(&id);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(&filename);
        std::fs::write(&path, b"cached result").unwrap();
        db.publish(
            &crate::audit::FileRecord {
                id: &id,
                file_type: "mp4",
                uploaded_at: &crate::audit::now(),
                uploader_ip: Some("192.0.2.1".parse().unwrap()),
                original_size: 0,
                stored_size: 13,
            },
            || Ok(()),
            || {},
        )
        .unwrap();
        let url = format!("/api/files/{date}/{filename}");
        (root, db, id, url, path)
    }

    #[test]
    fn sha256_and_settings_separate_results() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input");
        std::fs::write(&path, b"abc").unwrap();
        let hash = file_hash(&path).unwrap();
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(key("image", &hash, "png"), key("image", &hash, "jpg"));
        assert_ne!(
            key("resize", &hash, "80x80"),
            key("resize", &hash, "100x100")
        );
        assert_ne!(key("mute", &hash, ""), key("extract-audio", &hash, ""));
        assert_ne!(
            key("download-mp4", "url", "720"),
            key("download-mp4", "url", "1080")
        );
        assert_ne!(
            key("download-mp4", "url", "192"),
            key("download-mp3", "url", "192")
        );
        assert_ne!(key("a", "bc", "d"), key("ab", "c", "d"));
    }

    #[test]
    fn cache_survives_restart_and_tracks_reuse_without_counting_downloads() {
        let (root, db, id, url, _) = fixture();
        db.cache_result("key", &url).unwrap();
        drop(db);
        let db = crate::audit::Audit::open(&root.path().join("audit.sqlite3")).unwrap();
        let ip = "192.0.2.2".parse().unwrap();
        for _ in 0..2 {
            assert_eq!(
                db.cached_result("key", root.path(), ip).unwrap(),
                Some(url.clone())
            );
        }
        assert!(
            db.cached_result("other-settings", root.path(), ip)
                .unwrap()
                .is_none()
        );
        let connection = db.connection.lock().unwrap();
        let requests: i64 = connection
            .query_row(
                "SELECT request_count FROM cache_submissions WHERE file_id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(requests, 2);
        let downloads: i64 = connection
            .query_row(
                "SELECT request_count FROM files WHERE id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(downloads, 0);
        connection
            .execute("DELETE FROM files WHERE id=?1", [&id])
            .unwrap();
        for table in ["result_cache", "cache_submissions"] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn expired_missing_and_archived_files_are_not_reused() {
        for state in ["expired", "missing", "archived"] {
            let (root, db, id, url, path) = fixture();
            db.cache_result("key", &url).unwrap();
            match state {
                "expired" => {
                    let file = std::fs::File::options().write(true).open(path).unwrap();
                    file.set_times(
                        std::fs::FileTimes::new()
                            .set_modified(SystemTime::now() - crate::storage::RETENTION),
                    )
                    .unwrap();
                }
                "missing" => std::fs::remove_file(path).unwrap(),
                _ => {
                    db.connection
                        .lock()
                        .unwrap()
                        .execute("UPDATE files SET archived=1 WHERE id=?1", [&id])
                        .unwrap();
                }
            }
            assert!(
                db.cached_result("key", root.path(), "192.0.2.2".parse().unwrap())
                    .unwrap()
                    .is_none(),
                "{state}"
            );
            let count: i64 = db
                .connection
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM result_cache", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn result_paths_reject_traversal() {
        for url in [
            "https://example.com/a",
            "/api/files/09102026/../../secret.mp4",
            "/api/files/31022026/invalid.mp4",
            "/api/files/09102026/source.mp4",
        ] {
            assert!(result_path(Path::new("storage"), url).is_none());
        }
    }

    #[tokio::test]
    async fn same_key_waits_and_failed_worker_releases_lock() {
        let first = lock("concurrent-result").await;
        let waiter = tokio::spawn(async { lock("concurrent-result").await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        let unrelated =
            tokio::time::timeout(std::time::Duration::from_secs(1), lock("unrelated-result"))
                .await
                .unwrap();
        drop(first);
        let second = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        drop((second, unrelated));
        drop(
            tokio::time::timeout(std::time::Duration::from_secs(1), lock("concurrent-result"))
                .await
                .unwrap(),
        );
    }
}
