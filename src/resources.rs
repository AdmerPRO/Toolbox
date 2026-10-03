use axum::http::StatusCode;
use std::sync::{Arc, LazyLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

// Shared by uploads, blocking image workers, YouTube processes and ZIP maintenance.
static JOBS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));
pub fn acquire() -> Result<OwnedSemaphorePermit, (StatusCode, String)> {
    JOBS.clone().try_acquire_owned().map_err(|_| {
        (
            StatusCode::TOO_MANY_REQUESTS,
            "The server is busy. Please try again shortly.".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_jobs_and_releases_capacity() {
        let first = acquire().unwrap();
        let second = acquire().unwrap();
        assert_eq!(acquire().unwrap_err().0, StatusCode::TOO_MANY_REQUESTS);
        drop(first);
        let replacement = acquire().unwrap();
        drop((second, replacement));
    }
}
