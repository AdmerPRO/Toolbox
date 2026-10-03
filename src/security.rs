use axum::{
    extract::Request,
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};

pub async fn headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' https://i.ytimg.com https://img.youtube.com data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
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
    if api {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    }
    response
}
