use std::collections::HashSet;

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::{http::header::ContentType, web::Data, HttpResponse};
use lazy_static::lazy_static;
use prometheus::{Encoder, TextEncoder};
use tracing::Span;
use tracing_actix_web::{DefaultRootSpanBuilder, RootSpanBuilder};

use crate::api::NO_TELEMETRY_ENDPOINTS;
use crate::constants::HEADER_BRUPOP_NODE_NAME;

// tracing-actix-web doesn't provide a convenient way to remove any routes from the logs, so we use a global
// settings containing API paths to generate empty `tracing::Span`s on paths which we don't want logged.
lazy_static! {
    static ref EXCLUDED_PATHS: HashSet<String> = {
        let mut excluded = HashSet::new();
        for endpoint in NO_TELEMETRY_ENDPOINTS {
            excluded.insert(endpoint.to_string());
        }
        excluded
    };
}

#[derive(Default)]
pub(crate) struct BrupopApiserverRootSpanBuilder;

impl RootSpanBuilder for BrupopApiserverRootSpanBuilder {
    fn on_request_start(request: &ServiceRequest) -> Span {
        if EXCLUDED_PATHS.get(request.path()).is_none() {
            // Indicate that a `node_name` will be added to the span.
            request
                .headers()
                .get(HEADER_BRUPOP_NODE_NAME)
                .and_then(|node_name| node_name.to_str().ok())
                .map(|node_name| tracing_actix_web::root_span!(request, node_name))
                .unwrap_or_else(|| {
                    tracing_actix_web::root_span!(request, node_name = tracing::field::Empty)
                })
        } else {
            Span::none()
        }
    }

    fn on_request_end<B: MessageBody>(
        span: Span,
        response: &std::result::Result<ServiceResponse<B>, actix_web::Error>,
    ) {
        DefaultRootSpanBuilder::on_request_end(span, response);
    }
}

/// Custom error handler for OpenTelemetry metrics encoding errors
fn handle_metrics_error(err: prometheus::Error) {
    tracing::error!("Metrics encoding error: {}", err);
}

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
