use tracing::{info, warn};

pub async fn healthcheck_handler() -> &'static str {
    let status = "ok";
    info!(status = status, endpoint = "/api/healthcheck", "Healthcheck");
    
    "OK"
}