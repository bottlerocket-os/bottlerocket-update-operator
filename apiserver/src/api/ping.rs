use actix_web::{HttpResponse, Responder};

/// HTTP endpoint that implements a shallow health check for the HTTP service.
pub(crate) async fn health_check() -> impl Responder {
    HttpResponse::Ok().body("pong")
}
