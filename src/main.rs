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
                .unwrap_or_else(|_| "my_app=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Loading server...");

    let host = env::var("ADDRESS").context("No host address in .env")?;
    let port = env::var("PORT").context("No port number in .env")?;

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

    // let result = download_youtube_mp4::download_youtube_mp4("https://www.youtube.com/watch?v=IxX_QHay02M", 1024).await; // Epilepsy Warning - so its for testing youtube download i will delete it later (i think)
    //match result {
    //Ok(path) => println!("Path: {}", path.display()),
    //Err(e) => println!("Error: {}", e),
    //}
    axum::serve(listener, app).await?;

    Ok(())
}
