use crate::{
    auth::{K8STokenAuthorizor, K8STokenReviewer, TokenAuthMiddleware},
    constants::{
        HEADER_BRUPOP_K8S_AUTH_TOKEN, HEADER_BRUPOP_NODE_NAME, HEADER_BRUPOP_NODE_UID,
        NODE_RESOURCE_ENDPOINT,
    },
    error::{self, Result},
    telemetry,
};
use models::constants::{
    AGENT, APISERVER_HEALTH_CHECK_ROUTE, APISERVER_SERVICE_NAME, LABEL_COMPONENT, NAMESPACE,
};
use models::node::{BottlerocketNodeClient, BottlerocketNodeSelector, BottlerocketNodeStatus};

use actix_web::{
    http::HeaderMap,
    web::{self, Data},
    App, HttpRequest, HttpResponse, HttpServer, Responder,
};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, ListParams},
    runtime::{reflector, utils::try_flatten_touched, watcher::watcher},
    ResourceExt,
};
use serde_json::json;
use snafu::{OptionExt, ResultExt};
use tracing::{event, Level};
use tracing_actix_web::TracingLogger;

use std::convert::TryFrom;

/// A struct containing information intended to be passed to the apiserver via HTTP headers.
pub(crate) struct ApiserverCommonHeaders {
    pub node_selector: BottlerocketNodeSelector,
    pub k8s_auth_token: String,
}

/// Returns a string value extracted from HTTP headers.
fn extract_header_string(headers: &HeaderMap, key: &'static str) -> Result<String> {
    Ok(headers
        .get(key)
        .context(error::HTTPHeaderParse {
            missing_header: key,
        })?
        .to_str()
        .map_err(|_| error::Error::HTTPHeaderParse {
            missing_header: key,
        })?
        .to_string())
}

impl TryFrom<&HeaderMap> for ApiserverCommonHeaders {
    type Error = error::Error;

    fn try_from(headers: &HeaderMap) -> Result<Self> {
        let node_name = extract_header_string(headers, HEADER_BRUPOP_NODE_NAME)?;
        let node_uid = extract_header_string(headers, HEADER_BRUPOP_NODE_UID)?;
        let k8s_auth_token = extract_header_string(headers, HEADER_BRUPOP_K8S_AUTH_TOKEN)?;

        Ok(ApiserverCommonHeaders {
            node_selector: BottlerocketNodeSelector {
                node_name,
                node_uid,
            },
            k8s_auth_token,
        })
    }
}

// The set of API endpoints for which `tracing::Span`s will not be recorded.
pub const NO_TELEMETRY_ENDPOINTS: &[&str] = &[APISERVER_HEALTH_CHECK_ROUTE];

/// HTTP endpoint that implements a shallow health check for the HTTP service.
async fn health_check() -> impl Responder {
    HttpResponse::Ok().body("pong")
}

/// HTTP endpoint which creates BottlerocketNode custom resources on behalf of the caller.
async fn create_bottlerocket_node_resource<T: BottlerocketNodeClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    let br_node = settings
        .node_client
        .create_node(&headers.node_selector)
        .await
        .context(error::BottlerocketNodeCreate)?;

    Ok(HttpResponse::Ok().body(format!("{}", json!(&br_node))))
}

/// HTTP endpoint which updates the `status` of a BottlerocketNode custom resource on behalf of the caller.
async fn update_bottlerocket_node_resource<T: BottlerocketNodeClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
    node_status: web::Json<BottlerocketNodeStatus>,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    settings
        .node_client
        .update_node_status(&headers.node_selector, &node_status)
        .await
        .context(error::BottlerocketNodeUpdate)?;

    Ok(HttpResponse::Ok().body(format!("{}", json!(&node_status))))
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
    k8s_client: kube::Client,
) -> Result<()> {
    let server_port = settings.server_port;

    // Set up a reflector to watch all kubernetes pods in the namespace.
    // We use this information to authenticate write requests from brupop agents.
    let pods = Api::<Pod>::namespaced(k8s_client.clone(), NAMESPACE);

    let pod_store = reflector::store::Writer::<Pod>::default();
    let pod_reader = pod_store.as_reader();

    let pod_reflector = reflector::reflector(
        pod_store,
        watcher(
            pods,
            ListParams::default().labels(&format!("{}={}", LABEL_COMPONENT, AGENT)),
        ),
    );
    let drainer = try_flatten_touched(pod_reflector)
        .filter_map(|x| async move {
            if let Err(err) = &x {
                event!(Level::ERROR, %err, "Failed to process a Pod event");
            }
            std::result::Result::ok(x)
        })
        .for_each(|pod| {
            event!(Level::TRACE, pod_name = %pod.name(), ?pod.spec, ?pod.status, "Processed event for Pod");
            futures::future::ready(())
        });

    // Set up the actix server.
    let server = HttpServer::new(move || {
        App::new()
            .wrap(
                TokenAuthMiddleware::new(K8STokenAuthorizor::new(
                    K8STokenReviewer::new(k8s_client.clone()),
                    NAMESPACE.to_string(),
                    pod_reader.clone(),
                    Some(vec![APISERVER_SERVICE_NAME.to_string()]),
                ))
                .exclude(APISERVER_HEALTH_CHECK_ROUTE),
            )
            .wrap(TracingLogger::<telemetry::BrupopApiserverRootSpanBuilder>::new())
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
    .run();

    tokio::select! {
        _ = drainer => {
            event!(Level::ERROR, "reflector drained");
            return Err(error::Error::KubernetesWatcherFailed {});
        },
        res = server => {
            event!(Level::ERROR, "server exited");
            res.context(error::HttpServerError)?;
        },
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use models::constants::APISERVER_INTERNAL_PORT;
    use models::node::{
        BottlerocketNode, BottlerocketNodeSelector, BottlerocketNodeSpec, BottlerocketNodeState,
        BottlerocketNodeStatus, MockBottlerocketNodeClient, Version,
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
        let node_name = "test-node-name";
        let node_uid = "test-node-uid";

        let node_selector = BottlerocketNodeSelector {
            node_name: node_name.to_string(),
            node_uid: node_uid.to_string(),
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
            .insert_header((HEADER_BRUPOP_K8S_AUTH_TOKEN, "authy"))
            .insert_header((HEADER_BRUPOP_NODE_NAME, node_name))
            .insert_header((HEADER_BRUPOP_NODE_UID, node_uid))
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
        let node_name = "test-node-name";
        let node_uid = "test-node-uid";

        let node_selector = BottlerocketNodeSelector {
            node_name: node_name.to_string(),
            node_uid: node_uid.to_string(),
        };
        let node_status = BottlerocketNodeStatus::new(
            Version::new(1, 2, 1),
            Version::new(1, 3, 0),
            BottlerocketNodeState::default(),
        );

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
            .insert_header((HEADER_BRUPOP_K8S_AUTH_TOKEN, "authy"))
            .insert_header((HEADER_BRUPOP_NODE_NAME, node_name))
            .insert_header((HEADER_BRUPOP_NODE_UID, node_uid))
            .set_json(&node_status)
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
