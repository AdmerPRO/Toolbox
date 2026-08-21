mod api;
mod routes;
mod utils;

use api::{healthcheck, download_youtube_mp4};
use routes::root;

use anyhow::{Context, Result};
use axum::{routing::get, Router};
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
        .route("/api/healthcheck", get(healthcheck::healthcheck_handler))
        .fallback_service(ServeDir::new("frontend"))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .context("Failed connecting to address")?;

    println!("Server running on http://{}", bind_address);

    let result = download_youtube_mp4::download_youtube_mp4("https://www.youtube.com/watch?v=IxX_QHay02M", 1024).await;
    match result {
        Ok(path) => println!("Path: {}", path.display()),
        Err(e) => println!("Error: {}", e),
    }
    axum::serve(listener, app).await?;

    Ok(())
}