use tracing::{info, warn};

pub async fn root_handler() -> &'static str {
    info!("Main site handler");
    "Hello!!"
}