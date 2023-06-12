//! This module provides middleware for authenticating and authorizing requests from brupop agents to make changes to
//! their Node's resources (including BottlerocketShadow custom resources, or Draining their host Nodes of Pods.)
use super::TokenAuthorizor;
use crate::api::ApiserverCommonHeaders;

use actix_web::{
    body::MessageBody,
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
};

use std::{
    collections::HashSet,
    convert::TryFrom,
    future::{ready, Future, Ready},
    pin::Pin,
    rc::Rc,
};

// Per the actix-web documentation, there are two steps in middleware processing:
// * Middleware is initialized. A middleware factory is called with the next service in the chain as a parameter.
// * The middleware's call method is called with the request.

/// Middleware which checks that callers are brupop agents originating from the Nodes for which they are trying to make requests.
#[derive(Clone)]
pub struct TokenAuthMiddleware<T: TokenAuthorizor> {
    authorizor: T,
    exclude_paths: Rc<HashSet<String>>,
}

impl<T: TokenAuthorizor> TokenAuthMiddleware<T> {
    pub fn new(authorizor: T) -> Self {
        Self {
            authorizor,
            exclude_paths: Rc::new(HashSet::new()),
        }
    }

    pub fn exclude<S: Into<String>>(mut self, path: S) -> Self {
        // `exclude_paths` is a non-public member and cannot have multiple concurrent mutable references. This unwrap is safe.
        Rc::get_mut(&mut self.exclude_paths)
            .unwrap()
            .insert(path.into());
        self
    }
}

// Middleware factory is `Transform` trait.
// `S` - type of the next service
// `B` - type of response's body
impl<S, B, T> Transform<S, ServiceRequest> for TokenAuthMiddleware<T>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error>,
    S::Future: 'static,
    B: MessageBody + 'static,
    T: TokenAuthorizor + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = actix_web::Error;
    type InitError = ();
    type Transform = InnerTokenAuthMiddleware<S, T>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(InnerTokenAuthMiddleware {
            service,
            authorizor: self.authorizor.clone(),
            exclude_paths: self.exclude_paths.clone(),
        }))
    }
}

pub struct InnerTokenAuthMiddleware<S, T: TokenAuthorizor> {
    service: S,
    authorizor: T,
    exclude_paths: Rc<HashSet<String>>,
}

impl<S, B, T> Service<ServiceRequest> for InnerTokenAuthMiddleware<S, T>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error>,
    S::Future: 'static,
    B: MessageBody + 'static,
    T: TokenAuthorizor + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = actix_web::Error;
    #[allow(clippy::type_complexity)]
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    actix_web::dev::forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        // The future we return must be `static, so we need to pull information from self now.
        let maybe_apiserver_headers = ApiserverCommonHeaders::try_from(req.headers());

        // Clone the request path out of the request, since we're going to move it to our future.
        let request_path = req.path().to_string();

        let fut = self.service.call(req);
        let authorizor = self.authorizor.clone();

        if self.exclude_paths.get(&request_path).is_some() {
            Box::pin(async move { fut.await })
        } else {
            Box::pin(async move {
                let apiserver_headers = maybe_apiserver_headers?;
                authorizor
                    .check_request_authorized(
                        &apiserver_headers.node_selector,
                        &apiserver_headers.k8s_auth_token,
                    )
                    .await?;
                fut.await
            })
        }
    }
}

#[cfg(test)]
mod test {
    use super::super::authorizor::{
        mock::MockTokenReviewer,
        test::{fake_pod_named, fake_token_authorizor},
        POD_NAME_INFO_KEY,
    };
    use super::*;
    use crate::constants::{
        HEADER_BRUPOP_K8S_AUTH_TOKEN, HEADER_BRUPOP_NODE_NAME, HEADER_BRUPOP_NODE_UID,
    };

    use actix_web::{test, web, App, HttpResponse, Responder};
    use k8s_openapi::api::{
        authentication::v1::{TokenReview, TokenReviewSpec, TokenReviewStatus, UserInfo},
        core::v1::Pod,
    };
    use maplit::btreemap;

    async fn test_route() -> impl Responder {
        HttpResponse::Ok().body("Hello, world")
    }

    const TEST_URI: &str = "/hello";

    // Generates pods1-5
    // All pods live on a node with the same number, e.g. pod1 lives on node1.
    fn fake_pods() -> Vec<Pod> {
        (1..5)
            .map(|ndx| fake_pod_named(format!("pod{}", ndx), format!("node{}", ndx)))
            .collect()
    }

    // Mockall doesn't allow for `Clone` implementations appropriately, so we define our own cloner here.
    fn mock_reviewer_gen(
        expected_review: &TokenReview,
        ret_status: &TokenReviewStatus,
    ) -> MockTokenReviewer {
        let mut token_reviewer = MockTokenReviewer::new();

        // Middleware gets cloned by actix. When it's called, just replicate our existing expectations into the clone.
        let clone_review = expected_review.clone();
        let clone_status = ret_status.clone();
        token_reviewer
            .expect_clone()
            .returning(move || mock_reviewer_gen(&clone_review, &clone_status));

        let create_review = expected_review.clone();
        let create_status = ret_status.clone();
        token_reviewer
            .expect_create_token_review()
            .with(mockall::predicate::eq(create_review))
            .return_const(Ok(create_status));

        token_reviewer
    }

    #[tokio::test]
    async fn test_middleware_successful_auth() {
        // Set up a fake cluster environment and some mock adapters to connect to it.
        let requester_audiences = vec!["api-server".to_string()];
        let server_audiences = vec!["api-server".to_string()];

        // Our TokenReviewer assumes we have used the auth key for "pod1"
        let test_pod_name = "pod1";
        let node_name = "node1";
        let node_uid = "node1uid";
        let auth_token = "authy";

        // Assert that we're called with our server audiences and the given auth key
        let token_reviewer = mock_reviewer_gen(
            &TokenReview {
                spec: TokenReviewSpec {
                    token: Some(auth_token.to_string()),
                    audiences: Some(server_audiences.clone()),
                },
                ..Default::default()
            },
            // Return a status containing our reference pod + TokenReview metadata
            &TokenReviewStatus {
                audiences: Some(requester_audiences.clone()),
                authenticated: Some(true),
                error: None,
                user: Some(UserInfo {
                    extra: Some(btreemap! {
                        POD_NAME_INFO_KEY.to_string() => vec![test_pod_name.to_string()],
                    }),
                    ..Default::default()
                }),
            },
        );

        let authorizor = fake_token_authorizor(
            token_reviewer,
            "namespace",
            fake_pods(),
            Some(server_audiences.clone()),
        );

        let app = test::init_service(
            App::new()
                .route(TEST_URI, web::get().to(test_route))
                .wrap(TokenAuthMiddleware::new(authorizor)),
        )
        .await;

        // auth_token is configured to return being owned by `pod1`
        // `pod1` lives on `node1`
        // Our audiences are configured to intersect
        // No errors configured.
        // This should succeed.
        let req = test::TestRequest::get()
            .uri(TEST_URI)
            .insert_header((HEADER_BRUPOP_K8S_AUTH_TOKEN, auth_token))
            .insert_header((HEADER_BRUPOP_NODE_NAME, node_name))
            .insert_header((HEADER_BRUPOP_NODE_UID, node_uid))
            .to_request();

        let resp = app.call(req).await;
        assert!(resp.is_ok());
    }

    #[tokio::test]
    async fn test_middleware_unsuccessful_wrong_node() {
        // Set up a fake cluster environment and some mock adapters to connect to it.
        let requester_audiences = vec!["api-server".to_string()];
        let server_audiences = vec!["api-server".to_string()];

        // Our TokenReviewer assumes we have used the auth key for "pod1"
        // But our request will be comeing from "node2"
        let test_pod_name = "pod1";
        let node_name = "node2";
        let node_uid = "node2uid";
        let auth_token = "authy";

        // Assert that we're called with our server audiences and the given auth key
        let token_reviewer = mock_reviewer_gen(
            &TokenReview {
                spec: TokenReviewSpec {
                    token: Some(auth_token.to_string()),
                    audiences: Some(server_audiences.clone()),
                },
                ..Default::default()
            },
            // Return a status containing our reference pod + TokenReview metadata
            &TokenReviewStatus {
                audiences: Some(requester_audiences.clone()),
                authenticated: Some(true),
                error: None,
                user: Some(UserInfo {
                    extra: Some(btreemap! {
                        POD_NAME_INFO_KEY.to_string() => vec![test_pod_name.to_string()],
                    }),
                    ..Default::default()
                }),
            },
        );

        let authorizor = fake_token_authorizor(
            token_reviewer,
            "namespace",
            fake_pods(),
            Some(server_audiences.clone()),
        );

        let app = test::init_service(
            App::new()
                .route(TEST_URI, web::get().to(test_route))
                .wrap(TokenAuthMiddleware::new(authorizor)),
        )
        .await;

        // auth_token is configured to return being owned by `pod1`
        // `pod1` lives on `node1`
        // our request comes from `node2`
        // Our audiences are configured to intersect
        // No errors configured.
        // This should fail due to node mismatch.
        let req = test::TestRequest::get()
            .uri(TEST_URI)
            .insert_header((HEADER_BRUPOP_K8S_AUTH_TOKEN, auth_token))
            .insert_header((HEADER_BRUPOP_NODE_NAME, node_name))
            .insert_header((HEADER_BRUPOP_NODE_UID, node_uid))
            .to_request();

        let resp = app.call(req).await;

        assert!(resp.is_err());
    }

    #[tokio::test]
    async fn test_middleware_unsuccessful_server_error() {
        // Set up a fake cluster environment and some mock adapters to connect to it.
        let requester_audiences = vec!["api-server".to_string()];
        let server_audiences = vec!["api-server".to_string()];

        // Our TokenReviewer assumes we have used the auth key for "pod1"
        // But our request will be comeing from "node2"
        let test_pod_name = "pod1";
        let node_name = "node1";
        let node_uid = "node1uid";
        let auth_token = "authy";

        // Assert that we're called with our server audiences and the given auth key
        let token_reviewer = mock_reviewer_gen(
            &TokenReview {
                spec: TokenReviewSpec {
                    token: Some(auth_token.to_string()),
                    audiences: Some(server_audiences.clone()),
                },
                ..Default::default()
            },
            // Return a status containing our reference pod + TokenReview metadata
            &TokenReviewStatus {
                audiences: Some(requester_audiences.clone()),
                authenticated: Some(true),
                error: Some("ERROR".to_string()),
                user: Some(UserInfo {
                    extra: Some(btreemap! {
                        POD_NAME_INFO_KEY.to_string() => vec![test_pod_name.to_string()],
                    }),
                    ..Default::default()
                }),
            },
        );

        let authorizor = fake_token_authorizor(
            token_reviewer,
            "namespace",
            fake_pods(),
            Some(server_audiences.clone()),
        );

        let app = test::init_service(
            App::new()
                .route(TEST_URI, web::get().to(test_route))
                .wrap(TokenAuthMiddleware::new(authorizor)),
        )
        .await;

        // The TokenReviewer is returning an error. This will fail.
        let req = test::TestRequest::get()
            .uri(TEST_URI)
            .insert_header((HEADER_BRUPOP_K8S_AUTH_TOKEN, auth_token))
            .insert_header((HEADER_BRUPOP_NODE_NAME, node_name))
            .insert_header((HEADER_BRUPOP_NODE_UID, node_uid))
            .to_request();

        let resp = app.call(req).await;

        assert!(resp.is_err());
    }
}
