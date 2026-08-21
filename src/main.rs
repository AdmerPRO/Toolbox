mod api;
mod utils;
mod routes;

use anyhow::{Context, Result};
use axum::{routing::get, Router};
use std::env;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use tower_http::trace::TraceLayer;

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

    let host = env::var("ADDRESS").context("No host adress in .env")?;
    let port = env::var("PORT").context("No port number in .env")?;

    let bind_address = format!("{}:{}", host, port);

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/api/healthcheck", get(healthcheck_handler))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .context("Failed connecting to adress")?;

    println!("Server running on http://{}", bind_address);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn root_handler() -> &'static str {
    info!("Main site handler");
    "Hello!!"
}

async fn healthcheck_handler() -> &'static str {
    let status = "ok";
    info!(status = status, endpoint = "/api/healthcheck", "Healthcheck");
    
    "OK"
}