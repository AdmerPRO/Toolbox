mod api;
mod rate_limit;
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

    // Fail early if the canonical origin is misconfigured.
    root::site_url().map_err(|_| anyhow::anyhow!("Invalid SITE_URL configuration"))?;
    let limiter = rate_limit::RateLimiter::from_env()?;
    let mut app = Router::new()
        .route("/sitemap.xml", get(root::sitemap_handler))
        .route("/robots.txt", get(root::robots_handler))
        .route(
            "/resize/",
            get(|| async { root::page_handler("resize").await }),
        )
        .route("/mute/", get(|| async { root::page_handler("mute").await }))
        .route(
            "/api/convert/resize",
            post(api::media::resize_handler)
                .layer(DefaultBodyLimit::max(api::media::IMAGE_LIMIT + 64 * 1024)),
        )
        .route(
            "/api/convert/mute",
            post(api::media::mute_handler)
                .layer(DefaultBodyLimit::max(api::media::VIDEO_LIMIT + 64 * 1024)),
        )
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
        .route(
            "/images/",
            get(|| async { root::page_handler("images").await }),
        )
        .route(
            "/mp4tomp3/",
            get(|| async { root::page_handler("mp4tomp3").await }),
        )
        .route(
            "/privacy/",
            get(|| async { root::page_handler("privacy").await }),
        )
        .route("/", get(root::root_handler))
        .route_service("/style.css", ServeFile::new("frontend/root/style.css"))
        .route(
            "/youtubemp4/",
            get(|| async { root::page_handler("youtubemp4").await }),
        )
        .route("/api/healthcheck", get(healthcheck::healthcheck_handler))
        .route(
            "/youtubemp3/",
            get(|| async { root::page_handler("youtubemp3").await }),
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

    for (page, destination) in [
        ("root", "/"),
        ("images", "/images/"),
        ("resize", "/resize/"),
        ("mp4tomp3", "/mp4tomp3/"),
        ("mute", "/mute/"),
        ("youtubemp4", "/youtubemp4/"),
        ("youtubemp3", "/youtubemp3/"),
        ("privacy", "/privacy/"),
    ] {
        app = app.route(
            &format!("/{page}/index.html"),
            get(move || async move { axum::response::Redirect::permanent(destination) }),
        );
    }

    app = app.layer(axum::middleware::from_fn_with_state(
        limiter,
        rate_limit::middleware,
    ));
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .context("Failed connecting to address")?;

    println!("Server running on http://{}", bind_address);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}
