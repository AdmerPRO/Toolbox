use axum::{http::StatusCode, response::Html};
use tracing::info;

fn pasted_video_destination(uri: &axum::http::Uri) -> Option<String> {
    let raw = uri.path_and_query()?.as_str().strip_prefix('/')?;
    // Decode an encoded pasted URL without treating literal plus signs as spaces.
    let escaped = raw.replace('+', "%2B").replace('&', "%26").replace('=', "%3D");
    let input = url::form_urlencoded::parse(format!("url={escaped}").as_bytes())
        .next()?.1.into_owned();
    let canonical = crate::api::download_youtube_mp4::media_video_url(&input)?;
    let video = url::Url::parse(&canonical).ok()?;
    let (platform, watch) = match video.host_str()? {
        "www.youtube.com" => ("youtube", video.query_pairs().find(|(key, _)| key == "v")?.1.into_owned()),
        "www.instagram.com" => ("instagram", canonical),
        _ => ("tiktok", canonical),
    };
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("watch", &watch).finish();
    Some(format!("/{platform}/?{query}"))
}

pub async fn pasted_video_redirect(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        if let Some(destination) = pasted_video_destination(request.uri()) {
            return axum::response::Redirect::temporary(&destination).into_response();
        }
    }
    next.run(request).await
}

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
    fn pasted_links_route_to_their_platform() {
        for input in [
            "/https://www.youtube.com/watch?v=HZ1I2F5aBvc",
            "/https://youtu.be/HZ1I2F5aBvc?si=abc",
            "/https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DHZ1I2F5aBvc",
        ] {
            assert_eq!(pasted_video_destination(&input.parse().unwrap()).as_deref(), Some("/youtube/?watch=HZ1I2F5aBvc"));
        }
        for (input, prefix) in [
            ("/https://www.instagram.com/reel/ABC_123/?igsh=abc", "/instagram/?watch="),
            ("/https://www.tiktok.com/@user/video/123456", "/tiktok/?watch="),
            ("/https://vm.tiktok.com/ABC123/", "/tiktok/?watch="),
        ] {
            assert!(pasted_video_destination(&input.parse().unwrap()).unwrap().starts_with(prefix));
        }
        for input in ["/qr/", "/https://youtube.com.evil.example/watch?v=HZ1I2F5aBvc", "/https://example.com/video"] {
            assert!(pasted_video_destination(&input.parse().unwrap()).is_none());
        }
    }

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
