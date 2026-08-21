use tracing::info;

pub async fn healthcheck_handler() -> &'static str {
    let status = "ok";
    info!(status = status, endpoint = "/api/healthcheck", "Healthcheck requested");

    "OK"
}
