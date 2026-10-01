use axum::{http::StatusCode, response::Html};
use tracing::info;

pub async fn root_handler() -> Result<Html<String>, StatusCode> {
    info!("Main site handler");
    tokio::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/frontend/root/index.html"
    ))
    .await
    .map(Html)
    .map_err(|error| {
        tracing::error!(%error, "Cannot read home page");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}
