use crate::api::NO_TELEMETRY_ENDPOINTS;
use crate::constants::HEADER_BRUPOP_NODE_NAME;

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use lazy_static::lazy_static;
use tracing::Span;
use tracing_actix_web::{DefaultRootSpanBuilder, RootSpanBuilder};

use std::collections::HashSet;

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
