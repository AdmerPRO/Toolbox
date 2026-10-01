mod api;
mod routes;
mod utils;

use api::{download_youtube_mp4, healthcheck};
use routes::root;

use anyhow::{Context, Result};
use axum::{
    Router,
    routing::{get, post},
};
use std::env;
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "admersite=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Loading server...");

    let host = env::var("ADDRESS").unwrap_or_else(|_| "127.0.0.1".into());
    let port = env::var("PORT").unwrap_or_else(|_| "3000".into());

    let bind_address = format!("{}:{}", host, port);

    let app = Router::new()
        .route("/", get(root::root_handler))
        .route(
            "/youtubemp4/",
            get(|| async {
                axum::response::Html(include_str!("../frontend/youtubemp4/index.html"))
            }),
        )
        .route("/api/healthcheck", get(healthcheck::healthcheck_handler))
        .route(
            "/youtubemp3/",
            get(|| async {
                axum::response::Html(include_str!("../frontend/youtubemp3/index.html"))
            }),
        )
        .route(
            "/youtubemp3",
            get(|| async { axum::response::Redirect::permanent("/youtubemp3/") }),
        )
        .route(
            "/youtubemp4",
            get(|| async { axum::response::Redirect::permanent("/youtubemp4/") }),
        )
        .route(
            "/api/youtube/download/mp3",
            post(download_youtube_mp4::youtube_mp3_handler),
        )
        .route(
            "/api/youtube/info",
            post(download_youtube_mp4::youtube_info_handler),
        )
        .route(
            "/api/youtube/download",
            post(download_youtube_mp4::youtube_download_handler),
        )
        .route(
            "/api/youtube/file/{filename}",
            get(download_youtube_mp4::download_file_handler),
        )
        .fallback_service(ServeDir::new("frontend"))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .context("Failed connecting to address")?;

    println!("Server running on http://{}", bind_address);

    axum::serve(listener, app).await?;

    Ok(())
}
