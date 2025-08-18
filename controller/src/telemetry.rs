use actix_web::{get, http::header::ContentType, web::Data, HttpResponse};
use prometheus::{Encoder, TextEncoder};

/// Custom error handler for OpenTelemetry metrics encoding errors
fn handle_metrics_error(err: prometheus::Error) {
    tracing::error!("Metrics encoding error: {}", err);
}

#[get("/metrics")]
pub async fn vending_metrics(registry: Data<prometheus::Registry>) -> HttpResponse {
    let encoder = TextEncoder::new();
    let metric_families = registry.gather();
    let mut buf = Vec::new();

    match encoder.encode(&metric_families[..], &mut buf) {
        Ok(()) => {
            let body = String::from_utf8(buf).unwrap_or_default();
            HttpResponse::Ok()
                .insert_header(ContentType::plaintext())
                .body(body)
        }
        Err(err) => {
            handle_metrics_error(err);
            // Return empty metrics response on error
            HttpResponse::InternalServerError()
                .insert_header(ContentType::plaintext())
                .body("# Metrics encoding error\n")
        }
    }
}
