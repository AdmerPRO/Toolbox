mod api;
mod routes;
mod storage;
mod utils;

use api::{download_youtube_mp4, healthcheck};
use routes::root;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use std::env;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
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
    storage::start_archiver();

    let host = env::var("ADDRESS").unwrap_or_else(|_| "127.0.0.1".into());
    let port = env::var("PORT").unwrap_or_else(|_| "3000".into());

    let bind_address = format!("{}:{}", host, port);

    let app = Router::new()
        .route(
            "/api/convert/image",
            post(api::media::image_handler)
                .layer(DefaultBodyLimit::max(api::media::IMAGE_LIMIT + 64 * 1024)),
        )
        .route(
            "/api/convert/audio",
            post(api::media::audio_handler)
                .layer(DefaultBodyLimit::max(api::media::VIDEO_LIMIT + 64 * 1024)),
        )
        .route(
            "/api/files/{date}/{filename}",
            get(storage::download_handler),
        )
        .route_service(
            "/images/",
            ServeFile::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/frontend/images/index.html"
            )),
        )
        .route_service(
            "/mp4tomp3/",
            ServeFile::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/frontend/mp4tomp3/index.html"
            )),
        )
        .route_service(
            "/privacy/",
            ServeFile::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/frontend/privacy/index.html"
            )),
        )
        .route("/", get(root::root_handler))
        .route_service(
            "/style.css",
            ServeFile::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/frontend/root/style.css"
            )),
        )
        .route_service(
            "/youtubemp4/",
            ServeFile::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/frontend/youtubemp4/index.html"
            )),
        )
        .route("/api/healthcheck", get(healthcheck::healthcheck_handler))
        .route_service(
            "/youtubemp3/",
            ServeFile::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/frontend/youtubemp3/index.html"
            )),
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
        .fallback_service(ServeDir::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/frontend"
        )))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .context("Failed connecting to address")?;

    println!("Server running on http://{}", bind_address);

    axum::serve(listener, app).await?;

    Ok(())
}
