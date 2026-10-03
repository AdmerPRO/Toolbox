use axum::{http::StatusCode, response::Html};
use tracing::info;

pub async fn root_handler() -> Result<Html<String>, StatusCode> {
    page_handler("root").await
}

pub fn site_url() -> Result<String, StatusCode> {
    let value = std::env::var("SITE_URL").unwrap_or_else(|_| "https://tools.admerpro.com".into());
    validate_site_url(&value).map_err(|message| {
        tracing::error!(%message, "Invalid SITE_URL configuration");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

fn validate_site_url(value: &str) -> Result<String, &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "Use an absolute HTTP or HTTPS origin")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(
            "SITE_URL must be an HTTP or HTTPS origin without credentials, a path, query, or fragment",
        );
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn privacy_email() -> Result<String, StatusCode> {
    let email = std::env::var("PRIVACY_CONTACT_EMAIL")
        .unwrap_or_else(|_| "admin@tools.admerpro.com".into());
    if email.len() > 254
        || email.split('@').count() != 2
        || email.starts_with('@')
        || email.ends_with('@')
        || !email
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
    {
        tracing::error!("Invalid PRIVACY_CONTACT_EMAIL");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(email)
}

pub async fn page_handler(page: &str) -> Result<Html<String>, StatusCode> {
    let site = match page {
        "root" => "Main".to_owned(),
        _ => page.to_uppercase(),
    };
    info!("{site} site requested");
    let path = std::path::Path::new("frontend")
        .join(page)
        .join("index.html");
    let html = tokio::fs::read_to_string(path).await.map_err(|error| {
        tracing::error!(%error, "Cannot read page");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Html(
        html.replace("https://tools.admerpro.com", &site_url()?)
            .replace("{{PRIVACY_CONTACT_EMAIL}}", &privacy_email()?),
    ))
}

pub async fn sitemap_handler()
-> Result<([(axum::http::HeaderName, &'static str); 1], String), StatusCode> {
    info!("Sitemap requested");
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "application/xml; charset=utf-8",
        )],
        include_str!("../../frontend/sitemap.xml")
            .replace("https://tools.admerpro.com", &site_url()?),
    ))
}

pub async fn robots_handler()
-> Result<([(axum::http::HeaderName, &'static str); 1], String), StatusCode> {
    info!("Robots.txt requested");
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        include_str!("../../frontend/robots.txt")
            .replace("https://tools.admerpro.com", &site_url()?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_public_origin() {
        assert_eq!(
            validate_site_url("https://example.org/").unwrap(),
            "https://example.org"
        );
        assert_eq!(
            validate_site_url("http://localhost:8080").unwrap(),
            "http://localhost:8080"
        );
        for value in [
            "/relative",
            "ftp://example.org",
            "https://user:password@example.org",
            "https://example.org/path",
            "https://example.org/?query=1",
            "https://example.org/#fragment",
        ] {
            assert!(validate_site_url(value).is_err(), "{value}");
        }
    }
}
