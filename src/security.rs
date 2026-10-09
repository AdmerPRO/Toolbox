use axum::{
    extract::{Request, State},
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};

#[derive(Clone)]
pub struct SecurityHeaders {
    hsts: Option<HeaderValue>,
}
impl SecurityHeaders {
    pub fn from_env() -> anyhow::Result<Self> {
        let value = std::env::var("HSTS_MAX_AGE_SECONDS").unwrap_or_else(|_| "0".into());
        let origin =
            crate::routes::root::site_url().map_err(|_| anyhow::anyhow!("Invalid SITE_URL"))?;
        Ok(Self {
            hsts: hsts_value(&value, &origin)?,
        })
    }
}
fn hsts_value(value: &str, origin: &str) -> anyhow::Result<Option<HeaderValue>> {
    let seconds: u64 = value.parse()?;
    anyhow::ensure!(
        seconds <= 31536000,
        "HSTS_MAX_AGE_SECONDS must be between 0 and 31536000"
    );
    if seconds == 0 {
        return Ok(None);
    }
    anyhow::ensure!(
        origin.starts_with("https://"),
        "HSTS requires an HTTPS SITE_URL"
    );
    Ok(Some(HeaderValue::from_str(&format!("max-age={seconds}"))?))
}

pub async fn headers(
    State(config): State<SecurityHeaders>,
    request: Request,
    next: Next,
) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' https://i.ytimg.com https://img.youtube.com https://*.tiktokcdn.com https://*.tiktokcdn-us.com https://*.tiktokcdn-eu.com https://*.tiktok.com https://*.ibytedtos.com https://*.byteoversea.com https://*.cdninstagram.com https://*.fbcdn.net data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    if let Some(hsts) = config.hsts {
        response
            .headers_mut()
            .insert(header::STRICT_TRANSPORT_SECURITY, hsts);
    }
    if api {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn configured_hsts_is_added_to_error_responses() {
        use axum::{Router, body::Body};
        use tower::ServiceExt;
        let app = Router::new().layer(axum::middleware::from_fn_with_state(
            SecurityHeaders {
                hsts: hsts_value("86400", "https://example.com").unwrap(),
            },
            headers,
        ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers()[header::STRICT_TRANSPORT_SECURITY],
            "max-age=86400"
        );
    }

    #[test]
    fn hsts_is_explicit_and_requires_https() {
        assert!(hsts_value("0", "http://localhost:3000").unwrap().is_none());
        assert_eq!(
            hsts_value("31536000", "https://example.com")
                .unwrap()
                .unwrap(),
            "max-age=31536000"
        );
        assert!(hsts_value("3600", "http://localhost:3000").is_err());
        assert!(hsts_value("31536001", "https://example.com").is_err());
        assert!(hsts_value("invalid", "https://example.com").is_err());
    }
}
