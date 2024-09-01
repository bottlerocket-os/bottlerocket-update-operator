use actix_web::{get, http::header::ContentType, web::Data, HttpResponse};
use opentelemetry::{global, metrics::MetricsError};
use prometheus::{Encoder, TextEncoder};

#[get("/metrics")]
pub async fn vending_metrics(registry: Data<prometheus::Registry>) -> HttpResponse {
    let encoder = TextEncoder::new();
    let metric_families = registry.gather();
    let mut buf = Vec::new();
    if let Err(err) = encoder.encode(&metric_families[..], &mut buf) {
        global::handle_error(MetricsError::Other(err.to_string()));
    }

    let body = String::from_utf8(buf).unwrap_or_default();
    HttpResponse::Ok()
        .insert_header(ContentType::plaintext())
        .body(body)
}
