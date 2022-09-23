use super::{APIServerSettings, ApiserverCommonHeaders};
use crate::error::{self, Result};
use models::node::BottlerocketShadowClient;

use actix_web::{
    web::{self},
    HttpRequest, HttpResponse, Responder,
};
use snafu::ResultExt;

use std::convert::TryFrom;

/// HTTP endpoint which prevents work from being scheduled to a node, and drains all pods currently running.
pub(crate) async fn cordon_and_drain<T: BottlerocketShadowClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    settings
        .node_client
        .cordon_node(&headers.node_selector)
        .await
        .context(error::BottlerocketShadowCordonSnafu)?;

    settings
        .node_client
        .drain_node(&headers.node_selector)
        .await
        .context(error::BottlerocketShadowDrainSnafu)?;

    Ok(HttpResponse::Ok())
}

/// HTTP endpoint which re-allows work to be scheduled on a node that has been cordoned.
pub(crate) async fn uncordon<T: BottlerocketShadowClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    settings
        .node_client
        .uncordon_node(&headers.node_selector)
        .await
        .context(error::BottlerocketShadowCordonSnafu)?;

    Ok(HttpResponse::Ok())
}

/// HTTP endpoint which exludes a node from load balancer.
pub(crate) async fn exclude<T: BottlerocketShadowClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    settings
        .node_client
        .exclude_node_from_lb(&headers.node_selector)
        .await
        .context(error::BottlerocketShadowDrainSnafu)?;

    Ok(HttpResponse::Ok())
}

/// HTTP endpoint which remove node's exlusion from load balancer.
pub(crate) async fn remove_exclusion<T: BottlerocketShadowClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    settings
        .node_client
        .remove_node_exclusion_from_lb(&headers.node_selector)
        .await
        .context(error::BottlerocketShadowDrainSnafu)?;

    Ok(HttpResponse::Ok())
}
