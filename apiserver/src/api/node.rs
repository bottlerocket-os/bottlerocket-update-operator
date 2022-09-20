use super::{APIServerSettings, ApiserverCommonHeaders};
use crate::error::{self, Result};
use crate::webhook::ConversionRequest;

use models::node::{BottlerocketShadowClient, BottlerocketShadowStatus};

use actix_web::{
    web::{self},
    HttpRequest, HttpResponse, Responder,
};

use serde_json::json;
use snafu::ResultExt;
use std::convert::TryFrom;
use tracing::{event, Level};

/// HTTP endpoint which creates BottlerocketShadow custom resources on behalf of the caller.
pub(crate) async fn create_bottlerocket_shadow_resource<T: BottlerocketShadowClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    let br_node = settings
        .node_client
        .create_node(&headers.node_selector)
        .await
        .context(error::BottlerocketShadowCreateSnafu)?;

    Ok(HttpResponse::Ok().body(format!("{}", json!(&br_node))))
}

/// HTTP endpoint which updates the `status` of a BottlerocketShadow custom resource on behalf of the caller.
pub(crate) async fn update_bottlerocket_shadow_resource<T: BottlerocketShadowClient>(
    settings: web::Data<APIServerSettings<T>>,
    http_req: HttpRequest,
    node_status: web::Json<BottlerocketShadowStatus>,
) -> Result<impl Responder> {
    let headers = ApiserverCommonHeaders::try_from(http_req.headers())?;
    settings
        .node_client
        .update_node_status(&headers.node_selector, &node_status)
        .await
        .context(error::BottlerocketShadowUpdateSnafu)?;

    Ok(HttpResponse::Ok().body(format!("{}", json!(&node_status))))
}

pub(crate) async fn convert_bottlerocket_shadow_resource(
    conversion_req: web::Json<ConversionRequest>,
) -> Result<impl Responder> {
    event!(Level::INFO, ?conversion_req, "Original conversion request");
    let response = conversion_req.convert_resource();
    let response_string = serde_json::to_string(&response).context(error::WebhookSnafu)?;
    event!(Level::INFO, ?response_string, "Converted response:");

    // Webhook will always respond with 200.
    // The actual status of the conversion will be returned in
    // ConversionResponse.response.result.status
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body(response_string))
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_settings;
    use super::*;
    use crate::constants::{
        CRD_CONVERT_ENDPOINT, HEADER_BRUPOP_K8S_AUTH_TOKEN, HEADER_BRUPOP_NODE_NAME,
        HEADER_BRUPOP_NODE_UID, NODE_RESOURCE_ENDPOINT,
    };
    use crate::webhook::{ConversionRequest, ConversionResponse, Request};
    use models::node::{
        BottlerocketShadow, BottlerocketShadowSelector, BottlerocketShadowSpec,
        BottlerocketShadowState, MockBottlerocketShadowClient, Version,
    };

    use actix_web::{
        body::AnyBody,
        test,
        web::{self, Data},
        App,
    };
    use mockall::predicate;

    use std::sync::Arc;

    #[tokio::test]
    async fn test_create_node() {
        let node_name = "test-node-name";
        let node_uid = "test-node-uid";

        let node_selector = BottlerocketShadowSelector {
            node_name: node_name.to_string(),
            node_uid: node_uid.to_string(),
        };

        let return_value =
            BottlerocketShadow::new("brs-test-node-name", BottlerocketShadowSpec::default());
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

        let app = test::init_service(
            App::new()
                .route(
                    NODE_RESOURCE_ENDPOINT,
                    web::post().to(create_bottlerocket_shadow_resource::<
                        Arc<MockBottlerocketShadowClient>,
                    >),
                )
                .app_data(Data::new(settings)),
        )
        .await;

        let resp = test::call_service(&app, req).await;

        // The call returns a JSON-ified copy of the created node on success.
        assert!(resp.status().is_success());
        if let AnyBody::Bytes(b) = resp.into_body() {
            let brs: BottlerocketShadow =
                serde_json::from_slice(&b).expect("Could not parse JSON response.");
            assert_eq!(brs, expected_return_value);
        } else {
            panic!("Response did not return a body.");
        }
    }

    #[tokio::test]
    async fn test_update_node() {
        let node_name = "test-node-name";
        let node_uid = "test-node-uid";

        let node_selector = BottlerocketShadowSelector {
            node_name: node_name.to_string(),
            node_uid: node_uid.to_string(),
        };
        let node_status = BottlerocketShadowStatus::new(
            Version::new(1, 2, 1),
            Version::new(1, 3, 0),
            BottlerocketShadowState::default(),
            0,
            None,
        );

        let settings = test_settings(|node_client| {
            let my_selector = node_selector.clone();
            let my_status = node_status.clone();
            node_client
                .expect_update_node_status()
                .returning(|_, _| Ok(()))
                .withf(
                    move |selector: &BottlerocketShadowSelector,
                          status: &BottlerocketShadowStatus| {
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

        let app = test::init_service(
            App::new()
                .route(
                    NODE_RESOURCE_ENDPOINT,
                    web::put().to(update_bottlerocket_shadow_resource::<
                        Arc<MockBottlerocketShadowClient>,
                    >),
                )
                .app_data(Data::new(settings)),
        )
        .await;

        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
        if let AnyBody::Bytes(b) = resp.into_body() {
            let return_status: BottlerocketShadowStatus =
                serde_json::from_slice(&b).expect("Could not parse JSON response.");
            assert_eq!(return_status, node_status);
        } else {
            panic!("Response did not return a body.");
        }
    }

    #[tokio::test]
    async fn test_convert_crd() {
        let conversion_req = ConversionRequest {
            kind: "ConversionReview".to_string(),
            api_version: "apiextensions.k8s.io/v1".to_string(),
            request: Request {
                uid: "5a6adc7e-c74b-43c0-9718-293de1b104cb".to_string(),
                desired_api_version: "brupop.bottlerocket.aws/v2".to_string(),
                objects: vec![json!({
                    "apiVersion": "brupop.bottlerocket.aws/v1",
                    "kind": "BottlerocketShadow",
                    "metadata": {
                        "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                        "namespace": "brupop-bottlerocket-aws",
                        "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                        "ownerReferences": [
                            {
                                "apiVersion": "v1",
                                "kind": "Node",
                                "name": "ip-192-168-22-145.us-west-2.compute.internal",
                                "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                            }
                        ]
                    },
                    "spec": {
                        "state": "Idle",
                    },
                    "status": {
                        "current_state": "Idle",
                        "target_version": "1.8.0",
                        "current_version": "1.8.0"
                    }

                })],
            },
        };
        let req = test::TestRequest::put()
            .uri(CRD_CONVERT_ENDPOINT)
            .set_json(&conversion_req)
            .to_request();

        let app = test::init_service(App::new().route(
            CRD_CONVERT_ENDPOINT,
            web::put().to(convert_bottlerocket_shadow_resource),
        ))
        .await;

        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
        if let AnyBody::Bytes(b) = resp.into_body() {
            // Only check the response body can be converted to ConversionResponse.
            // Contents of the ConversionResponse should be tested in convert_resource method.
            serde_json::from_slice::<ConversionResponse>(&b)
                .expect("Could not parse JSON response.");
        } else {
            panic!("Response did not return a body.");
        }
    }
}
