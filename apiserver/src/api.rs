use crate::error::{self, Result};
use models::constants::APISERVER_HEALTH_CHECK_ROUTE;
use models::node::{BottlerocketNodeClient, BottlerocketNodeSelector, BottlerocketNodeStatus};

use actix_web::{
    middleware,
    web::{self, Data},
    App, HttpRequest, HttpResponse, HttpServer, Responder,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use snafu::ResultExt;

const NODE_RESOURCE_ENDPOINT: &'static str = "/bottlerocket-node-resource";

/// HTTP endpoint that implements a shallow health check for the HTTP service.
async fn health_check() -> impl Responder {
    HttpResponse::Ok().body("pong")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes a node for which a BottlerocketNode custom resource should be constructed.
pub struct CreateBottlerocketNodeRequest {
    pub node_selector: BottlerocketNodeSelector,
}

/// HTTP endpoint which creates BottlerocketNode custom resources on behalf of the caller.
async fn create_bottlerocket_node_resource<T: BottlerocketNodeClient>(
    _req: HttpRequest,
    settings: web::Data<APIServerSettings<T>>,
    create_request: web::Json<CreateBottlerocketNodeRequest>,
) -> Result<impl Responder> {
    let br_node = settings
        .node_client
        .create_node(&create_request.node_selector)
        .await
        .context(error::BottlerocketNodeCreate)?;

    Ok(HttpResponse::Ok().body(format!("{}", json!(&br_node))))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes updates to a BottlerocketNode object's `status`.
pub struct UpdateBottlerocketNodeRequest {
    pub node_selector: BottlerocketNodeSelector,
    pub node_status: BottlerocketNodeStatus,
}

/// HTTP endpoint which updates the `status` of a BottlerocketNode custom resource on behalf of the caller.
async fn update_bottlerocket_node_resource<T: BottlerocketNodeClient>(
    _req: HttpRequest,
    settings: web::Data<APIServerSettings<T>>,
    update_request: web::Json<UpdateBottlerocketNodeRequest>,
) -> Result<impl Responder> {
    settings
        .node_client
        .update_node_status(&update_request.node_selector, &update_request.node_status)
        .await
        .context(error::BottlerocketNodeUpdate)?;

    Ok(HttpResponse::Ok().body(format!("{}", json!(&update_request.node_status))))
}

#[derive(Clone)]
/// Settings that are applied to the apiserver. These settings are provided to each HTTP route
/// via actix's application data system.
pub struct APIServerSettings<T: BottlerocketNodeClient> {
    pub node_client: T,
    pub server_port: u16,
}

/// Runs the apiserver using the given settings.
pub async fn run_server<T: 'static + BottlerocketNodeClient>(
    settings: APIServerSettings<T>,
) -> Result<()> {
    let server_port = settings.server_port;
    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default().exclude(APISERVER_HEALTH_CHECK_ROUTE))
            .app_data(Data::new(settings.clone()))
            .service(
                web::resource(NODE_RESOURCE_ENDPOINT)
                    .route(web::post().to(create_bottlerocket_node_resource::<T>))
                    .route(web::put().to(update_bottlerocket_node_resource::<T>)),
            )
            .route(APISERVER_HEALTH_CHECK_ROUTE, web::get().to(health_check))
    })
    .bind(format!("0.0.0.0:{}", server_port))
    .context(error::HttpServerError)?
    .run()
    .await
    .context(error::HttpServerError)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::constants::APISERVER_INTERNAL_PORT;
    use models::node::{
        BottlerocketNode, BottlerocketNodeSpec, BottlerocketNodeState, MockBottlerocketNodeClient,
    };

    use actix_web::body::AnyBody;
    use actix_web::test;
    use mockall::predicate;

    use std::sync::Arc;

    fn test_settings<F>(mock_expectations: F) -> APIServerSettings<Arc<MockBottlerocketNodeClient>>
    where
        F: FnOnce(&mut MockBottlerocketNodeClient),
    {
        let mut node_client = MockBottlerocketNodeClient::new();
        mock_expectations(&mut node_client);

        // Construct an Arc around node_client so that we can share a reference to the
        // client used in the mock server.
        let node_client = Arc::new(node_client);

        APIServerSettings {
            node_client: node_client,
            server_port: APISERVER_INTERNAL_PORT as u16,
        }
    }

    #[tokio::test]
    async fn test_create_node() {
        let node_selector = BottlerocketNodeSelector {
            node_name: "test-node-name".to_string(),
            node_uid: "test-node-uid".to_string(),
        };

        let return_value =
            BottlerocketNode::new("brn-test-node-name", BottlerocketNodeSpec::default());
        let expected_return_value = return_value.clone();

        let settings = test_settings(|node_client| {
            node_client
                .expect_create_node()
                .returning(move |_| Ok(return_value.clone()))
                .with(predicate::eq(node_selector.clone()))
                .times(1);
        });

        let req = test::TestRequest::post()
            .uri(NODE_RESOURCE_ENDPOINT)
            .set_json(&CreateBottlerocketNodeRequest { node_selector })
            .to_request();

        let mut app = test::init_service(
            App::new()
                .route(
                    NODE_RESOURCE_ENDPOINT,
                    web::post()
                        .to(create_bottlerocket_node_resource::<Arc<MockBottlerocketNodeClient>>),
                )
                .app_data(Data::new(settings)),
        )
        .await;

        let resp = test::call_service(&mut app, req).await;

        // The call returns a JSON-ified copy of the created node on success.
        assert!(resp.status().is_success());
        if let AnyBody::Bytes(b) = resp.into_body() {
            let brn: BottlerocketNode =
                serde_json::from_slice(&b).expect("Could not parse JSON response.");
            assert_eq!(brn, expected_return_value);
        } else {
            panic!("Response did not return a body.");
        }
    }

    #[tokio::test]
    async fn test_update_node() {
        let node_selector = BottlerocketNodeSelector {
            node_name: "test-node-name".to_string(),
            node_uid: "test-node-uid".to_string(),
        };
        let node_status = BottlerocketNodeStatus {
            current_version: "1.2.1".to_string(),
            available_versions: vec!["1.3.0".to_string()],
            current_state: BottlerocketNodeState::default(),
        };

        let settings = test_settings(|node_client| {
            let my_selector = node_selector.clone();
            let my_status = node_status.clone();
            node_client
                .expect_update_node_status()
                .returning(|_, _| Ok(()))
                .withf(
                    move |selector: &BottlerocketNodeSelector, status: &BottlerocketNodeStatus| {
                        my_selector == selector.clone() && my_status == status.clone()
                    },
                )
                .times(1);
        });

        let req = test::TestRequest::put()
            .uri(NODE_RESOURCE_ENDPOINT)
            .set_json(&UpdateBottlerocketNodeRequest {
                node_selector: node_selector.clone(),
                node_status: node_status.clone(),
            })
            .to_request();

        let mut app = test::init_service(
            App::new()
                .route(
                    NODE_RESOURCE_ENDPOINT,
                    web::put()
                        .to(update_bottlerocket_node_resource::<Arc<MockBottlerocketNodeClient>>),
                )
                .app_data(Data::new(settings)),
        )
        .await;

        let resp = test::call_service(&mut app, req).await;

        assert!(resp.status().is_success());
        if let AnyBody::Bytes(b) = resp.into_body() {
            let return_status: BottlerocketNodeStatus =
                serde_json::from_slice(&b).expect("Could not parse JSON response.");
            assert_eq!(return_status, node_status);
        } else {
            panic!("Response did not return a body.");
        }
    }
}
