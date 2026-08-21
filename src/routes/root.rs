use axum::response::Html;
use tracing::info;

pub async fn root_handler() -> Html<&'static str> {
    info!("Main site handler");
    Html(include_str!("../../frontend/root/index.html"))
}
