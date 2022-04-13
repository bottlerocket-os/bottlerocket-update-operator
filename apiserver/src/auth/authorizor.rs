//! This module provides abstractions for authenticating and authorizing requests from brupop agents to make changes to
//! the underlying Node's resources (including BottlerocketShadow custom resources, or draining the host Nodes of Pods.)
use super::error::*;
use models::node::BottlerocketShadowSelector;

use async_trait::async_trait;
use k8s_openapi::api::{
    authentication::v1::{TokenReview, TokenReviewSpec, TokenReviewStatus},
    core::v1::Pod,
};
use kube::{
    api::{Api, PostParams},
    runtime::reflector::{ObjectRef, Store},
};
use snafu::OptionExt;
use tracing::instrument;

use std::collections::HashSet;

/// A token authorizor can determine if a given identity is authorized to make changes to a particular node.
#[async_trait]
pub trait TokenAuthorizor: Clone {
    /// Determine if the identity represented by the provided auth token has access to the provided node.
    async fn check_request_authorized(
        &self,
        node_selector: &BottlerocketShadowSelector,
        auth_token: &str,
    ) -> Result<(), AuthorizationError>;
}

// The k8s TokenReview authenticator adds the pod name in the `extra` field of the UserInfo
// provided with our TokenReview.status. This is the key to retrieve it.
//
// Presence in the `extra` field means that this is non-standard, so it's possible that brupop will not
// work with some implementations of a TokenReview server. The current Kubernetes API implementation seems to guarantee it.
pub const POD_NAME_INFO_KEY: &str = "authentication.kubernetes.io/pod-name";

#[derive(Clone)]
pub struct K8STokenAuthorizor<T: TokenReviewer> {
    token_reviewer: T,
    namespace: String,
    pod_reader: Store<Pod>,
    k8s_audiences: Option<Vec<String>>,
}

#[async_trait]
impl<T: TokenReviewer> TokenAuthorizor for K8STokenAuthorizor<T> {
    /// Returns Ok(()) if a write operation is permitted to this given node by the requester, and Err(_) otherwise.
    #[instrument(skip(self, auth_token))]
    async fn check_request_authorized(
        &self,
        node_selector: &BottlerocketShadowSelector,
        auth_token: &str,
    ) -> Result<(), AuthorizationError> {
        let token_review_req = TokenReview {
            spec: TokenReviewSpec {
                token: Some(auth_token.to_string()),
                audiences: self.k8s_audiences.clone(),
            },
            ..Default::default()
        };

        let review_status = self
            .token_reviewer
            .create_token_review(token_review_req)
            .await?;

        if let Some(err_msg) = review_status.error {
            return Err(AuthorizationError::TokenReviewServerError { err_msg });
        }

        // We are authorized under the conditions that:
        // * `review_status.authenticated` is Some(true)
        // * The intersection of `review_status.audiences` and `k8s_audiences` is not empty
        // * `review_status.extra` contains the pod name, and the referred pod is deployed to our target node.
        self.check_token_has_authenticated(&review_status)?;
        self.check_audiences_are_compatible(&review_status)?;
        self.check_requester_is_from_correct_node(&review_status, node_selector)
            .await?;

        Ok(())
    }
}

impl<T: TokenReviewer> K8STokenAuthorizor<T> {
    pub(crate) fn new(
        token_reviewer: T,
        namespace: String,
        pod_reader: Store<Pod>,
        k8s_audiences: Option<Vec<String>>,
    ) -> Self {
        K8STokenAuthorizor {
            token_reviewer,
            namespace,
            pod_reader,
            k8s_audiences,
        }
    }

    /// Returns whether or not the TokenReview server has authenticated the given token.
    fn check_token_has_authenticated(
        &self,
        token_review_status: &TokenReviewStatus,
    ) -> Result<(), AuthorizationError> {
        if token_review_status.authenticated == Some(true) {
            Ok(())
        } else {
            Err(AuthorizationError::TokenNotAuthenticated {})
        }
    }

    /// Returns Ok(()) if the Token owner and reviewer have compatible audience lists.
    fn check_audiences_are_compatible(
        &self,
        token_review_status: &TokenReviewStatus,
    ) -> Result<(), AuthorizationError> {
        // If we've been provided audiences, assert that we were returned audiences and the intersection is not empty.
        if let Some(provided_audiences) = self.k8s_audiences.as_ref() {
            if let Some(returned_audiences) = token_review_status.audiences.as_ref() {
                let lhs: HashSet<String> = provided_audiences.iter().cloned().collect();
                let rhs: HashSet<String> = returned_audiences.iter().cloned().collect();
                if lhs.intersection(&rhs).next().is_none() {
                    Err(AuthorizationError::AudienceMismatch {})
                } else {
                    Ok(())
                }
            } else {
                Err(AuthorizationError::TokenReviewMissingPodName {})
            }
        } else {
            // If we weren't provided audiences, then we aren't checking for a specific audience.
            Ok(())
        }
    }

    /// Returns Ok(()) if the token-owning pod is hosted on our target node.
    async fn check_requester_is_from_correct_node(
        &self,
        token_review_status: &TokenReviewStatus,
        node_selector: &BottlerocketShadowSelector,
    ) -> Result<(), AuthorizationError> {
        let pod_name = token_review_status
            .user
            .as_ref()
            .and_then(|user| user.extra.as_ref())
            .and_then(|extra| extra.get(POD_NAME_INFO_KEY))
            .and_then(|pod_names| pod_names.first())
            .context(TokenReviewMissingPodName)?;

        let pod_node_name = self
            .pod_reader
            .get(&ObjectRef::new(pod_name).within(&self.namespace))
            .and_then(|pod| (*pod).clone().spec)
            .and_then(|pod_spec| pod_spec.node_name)
            .context(NoSuchPod {
                pod_name: pod_name.to_string(),
            })?;

        if pod_node_name == node_selector.node_name {
            Ok(())
        } else {
            Err(AuthorizationError::RequesterTargetMismatch {
                requesting_node: pod_node_name,
                target_node: node_selector.node_name.clone(),
            })
        }
    }
}

/// A trait for posting token reviews to kubernetes.
///
/// Useful for creating fakes in test cases.
#[async_trait]
pub trait TokenReviewer: Clone + Sync + Send {
    async fn create_token_review(
        &self,
        token_review_req: TokenReview,
    ) -> Result<TokenReviewStatus, AuthorizationError>;
}

#[derive(Clone)]
pub struct K8STokenReviewer {
    pub k8s_client: kube::Client,
}

impl K8STokenReviewer {
    pub fn new(k8s_client: kube::Client) -> Self {
        Self { k8s_client }
    }
}

impl From<kube::Client> for K8STokenReviewer {
    fn from(k8s_client: kube::Client) -> Self {
        K8STokenReviewer::new(k8s_client)
    }
}

#[async_trait]
impl TokenReviewer for K8STokenReviewer {
    async fn create_token_review(
        &self,
        token_review_req: TokenReview,
    ) -> Result<TokenReviewStatus, AuthorizationError> {
        Ok(Api::all(self.k8s_client.clone())
            .create(&PostParams::default(), &token_review_req)
            .await
            .map_err(|err| AuthorizationError::TokenReviewCreate {
                err_msg: format!("{}", err),
            })?
            .status
            .context(TokenReviewMissingStatus {})?)
    }
}

#[cfg(any(feature = "mockall", test))]
pub mod mock {
    use super::*;
    use mockall::{mock, predicate::*};

    mock! {
        pub TokenReviewer {}
        #[async_trait]
        impl TokenReviewer for TokenReviewer {
            async fn create_token_review(
                &self,
                token_review_req: TokenReview,
            ) -> Result<TokenReviewStatus, AuthorizationError>;
        }

        impl Clone for TokenReviewer {
            fn clone(&self) -> Self;
        }
    }

    mock! {
        /// A Mock APIServerClient for use in tests.
        pub TokenAuthorizor {}
        #[async_trait]
        impl TokenAuthorizor for TokenAuthorizor {
            async fn check_request_authorized(
                &self,
                node_selector: &BottlerocketShadowSelector,
                auth_token: &str,
            ) -> Result<(), AuthorizationError>;
        }

        impl Clone for TokenAuthorizor {
            fn clone(&self) -> Self;
        }
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::mock::MockTokenReviewer;
    use super::*;

    use k8s_openapi::api::authentication::v1::UserInfo;
    use k8s_openapi::api::core::v1::PodSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use kube::runtime::reflector;
    use kube::runtime::watcher::Event;
    use maplit::btreemap;

    pub(crate) fn fake_token_authorizor(
        reviewer: MockTokenReviewer,
        namespace: &str,
        pods: Vec<Pod>,
        audiences: Option<Vec<String>>,
    ) -> K8STokenAuthorizor<MockTokenReviewer> {
        let mut pod_store = reflector::store::Writer::<Pod>::default();
        let pod_reader = pod_store.as_reader();
        pod_store.apply_watcher_event(&Event::Restarted(pods));

        K8STokenAuthorizor::new(reviewer, namespace.to_string(), pod_reader, audiences)
    }

    #[tokio::test]
    async fn test_token_has_authenticated() {
        let authorizor = fake_token_authorizor(MockTokenReviewer::new(), "namespace", vec![], None);
        let mut test_cases = vec![
            (
                TokenReviewStatus {
                    authenticated: Some(true),
                    ..Default::default()
                },
                true,
            ),
            (
                TokenReviewStatus {
                    authenticated: Some(false),
                    ..Default::default()
                },
                false,
            ),
            (
                TokenReviewStatus {
                    authenticated: None,
                    ..Default::default()
                },
                false,
            ),
        ];

        for (review_status, success) in test_cases.drain(..) {
            let result = authorizor.check_token_has_authenticated(&review_status);
            if success {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[tokio::test]
    async fn test_audiences_are_compatible() {
        let authorizor = fake_token_authorizor(
            MockTokenReviewer::new(),
            "namespace",
            vec![],
            Some(vec![
                "test-audience1".to_string(),
                "test-audience2".to_string(),
            ]),
        );

        let mut test_cases = vec![
            (
                TokenReviewStatus {
                    audiences: Some(vec!["test-audience1".to_string()]),
                    ..Default::default()
                },
                true,
            ),
            (
                TokenReviewStatus {
                    ..Default::default()
                },
                false,
            ),
            (
                TokenReviewStatus {
                    audiences: Some(vec!["nomatch".to_string()]),
                    ..Default::default()
                },
                false,
            ),
            (
                TokenReviewStatus {
                    audiences: Some(vec!["test-audience2".to_string()]),
                    ..Default::default()
                },
                true,
            ),
            (
                TokenReviewStatus {
                    audiences: Some(vec![
                        "test-audience2".to_string(),
                        "test-audience1".to_string(),
                    ]),
                    ..Default::default()
                },
                true,
            ),
        ];

        for (review_status, success) in test_cases.drain(..) {
            let result = authorizor.check_audiences_are_compatible(&review_status);
            if success {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
            }
        }
    }

    pub(crate) fn fake_pod_named(name: String, node_name: String) -> Pod {
        Pod {
            metadata: ObjectMeta {
                name: Some(name),
                ..Default::default()
            },
            spec: Some(PodSpec {
                node_name: Some(node_name),
                ..Default::default()
            }),
            status: None,
        }
    }

    fn review_for_pod(name: &str) -> TokenReviewStatus {
        TokenReviewStatus {
            user: Some(UserInfo {
                extra: Some(btreemap! {
                    POD_NAME_INFO_KEY.to_string() => vec![name.to_string()],
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn selector_with_name(name: &str) -> BottlerocketShadowSelector {
        BottlerocketShadowSelector {
            node_name: name.to_string(),
            node_uid: "fake".to_string(),
        }
    }

    #[tokio::test]
    async fn test_requester_from_correct_node() {
        let pods: Vec<Pod> = (1..5)
            .map(|ndx| fake_pod_named(format!("pod{}", ndx), format!("node{}", ndx)))
            .collect();

        let authorizor = fake_token_authorizor(MockTokenReviewer::new(), "namespace", pods, None);

        let mut test_cases = vec![
            (review_for_pod("pod1"), selector_with_name("node1"), true),
            (review_for_pod("pod1"), selector_with_name("node3"), false),
            (review_for_pod("pod4"), selector_with_name("node4"), true),
        ];

        for (review_status, node_selector, success) in test_cases.drain(..) {
            let result = authorizor
                .check_requester_is_from_correct_node(&review_status, &node_selector)
                .await;
            if success {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
            }
        }
    }
}
