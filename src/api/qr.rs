use axum::{
    Extension, Json,
    http::{StatusCode, header},
    response::IntoResponse,
};
use image::{DynamicImage, ImageFormat, Luma};
use std::io::Cursor;

#[derive(serde::Deserialize)]
pub struct QrRequest {
    text: String,
}

fn generate(text: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(!text.trim().is_empty(), "Enter text or a URL.");
    anyhow::ensure!(text.len() <= 2000, "Use at most 2000 UTF-8 bytes.");
    let code = qrcode::QrCode::new(text.as_bytes())?;
    let image = code.render::<Luma<u8>>().min_dimensions(512, 512).build();
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(image).write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

pub async fn qr_handler(
    Extension(client_permit): Extension<std::sync::Arc<crate::rate_limit::JobPermit>>,
    Json(request): Json<QrRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::info!("QR code generation requested");
    let permit = crate::resources::acquire()?;
    let png = tokio::task::spawn_blocking(move || {
        let _client_permit = client_permit;
        let _permit = permit;
        generate(&request.text)
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "QR generation failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "QR generation failed.".into(),
        )
    })?
    .map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "Enter text or a URL up to 2000 UTF-8 bytes.".into(),
        )
    })?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=qr-code.png",
            ),
        ],
        png,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_png_with_white_quiet_zone_and_rejects_invalid_input() {
        let png = generate("https://example.org/zażółć").unwrap();
        let image = image::load_from_memory(&png).unwrap().to_luma8();
        assert!(image.width() >= 512);
        assert_eq!(image.width(), image.height());
        assert_eq!(image.get_pixel(0, 0).0, [255]);
        assert!(image.pixels().any(|pixel| pixel.0 == [0]));
        assert!(generate("   ").is_err());
        assert!(generate(&"a".repeat(2001)).is_err());
        assert!(generate(&"a".repeat(2000)).is_ok());
    }
}
