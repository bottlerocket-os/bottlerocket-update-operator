use super::error::{self, Result};
use crate::{
    constants::{
        HEADER_BRUPOP_K8S_AUTH_TOKEN, HEADER_BRUPOP_NODE_NAME, HEADER_BRUPOP_NODE_UID,
        NODE_RESOURCE_ENDPOINT,
    },
    CreateBottlerocketNodeRequest, DrainAndCordonBottlerocketNodeRequest,
    UncordonBottlerocketNodeRequest, UpdateBottlerocketNodeRequest,
};
use models::{
    constants::{APISERVER_SERVICE_NAME, APISERVER_SERVICE_PORT, NAMESPACE},
    node::{BottlerocketNode, BottlerocketNodeSelector, BottlerocketNodeStatus},
};

use async_trait::async_trait;
use snafu::ResultExt;
use tokio::time::Duration;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};
use tracing::instrument;

// The web client uses exponential backoff.
// These values configure how long to delay between tries.
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(10);
const NUM_RETRIES: usize = 5;

fn retry_strategy() -> impl Iterator<Item = Duration> {
    ExponentialBackoff::from_millis(RETRY_BASE_DELAY.as_millis() as u64)
        .max_delay(RETRY_MAX_DELAY)
        .map(jitter)
        .take(NUM_RETRIES)
}

#[async_trait]
pub trait APIServerClient {
    async fn create_bottlerocket_node(
        &self,
        req: CreateBottlerocketNodeRequest,
    ) -> Result<BottlerocketNode>;
    async fn update_bottlerocket_node(
        &self,
        req: UpdateBottlerocketNodeRequest,
    ) -> Result<BottlerocketNodeStatus>;
    async fn drain_and_cordon_node(&self, req: DrainAndCordonBottlerocketNodeRequest)
        -> Result<()>;
    async fn uncordon_node(&self, req: UncordonBottlerocketNodeRequest) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct K8SAPIServerClient {
    k8s_auth_token: String,
}

impl K8SAPIServerClient {
    pub fn new(k8s_auth_token: String) -> Self {
        Self { k8s_auth_token }
    }

    pub fn scheme() -> String {
        "http".to_string()
    }

    pub fn server_domain() -> String {
        format!(
            "{}.{}.svc.cluster.local:{}",
            APISERVER_SERVICE_NAME, NAMESPACE, APISERVER_SERVICE_PORT
        )
    }

    fn add_common_request_headers(
        &self,
        req: reqwest::RequestBuilder,
        node_selector: &BottlerocketNodeSelector,
    ) -> reqwest::RequestBuilder {
        req.header(HEADER_BRUPOP_NODE_UID, &node_selector.node_uid)
            .header(HEADER_BRUPOP_NODE_NAME, &node_selector.node_name)
            .header(HEADER_BRUPOP_K8S_AUTH_TOKEN, &self.k8s_auth_token)
    }
}

#[async_trait]
impl APIServerClient for K8SAPIServerClient {
    #[instrument]
    async fn create_bottlerocket_node(
        &self,
        req: CreateBottlerocketNodeRequest,
    ) -> Result<BottlerocketNode> {
        Retry::spawn(retry_strategy(), || async {
            let http_client = reqwest::Client::new();

            let response = self
                .add_common_request_headers(
                    http_client.post(format!(
                        "{}://{}{}",
                        Self::scheme(),
                        Self::server_domain(),
                        NODE_RESOURCE_ENDPOINT
                    )),
                    &req.node_selector,
                )
                .json(&req)
                .send()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::CreateBottlerocketNodeResource {
                    selector: req.node_selector.clone(),
                })?;

            let node = response
                .json::<BottlerocketNode>()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::CreateBottlerocketNodeResource {
                    selector: req.node_selector.clone(),
                })?;

            Ok(node)
        })
        .await
    }

    #[instrument]
    async fn update_bottlerocket_node(
        &self,
        req: UpdateBottlerocketNodeRequest,
    ) -> Result<BottlerocketNodeStatus> {
        Retry::spawn(retry_strategy(), || async {
            let http_client = reqwest::Client::new();

            let response = self
                .add_common_request_headers(
                    http_client.put(format!(
                        "{}://{}{}",
                        Self::scheme(),
                        Self::server_domain(),
                        NODE_RESOURCE_ENDPOINT
                    )),
                    &req.node_selector,
                )
                .json(&req.node_status)
                .send()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::UpdateBottlerocketNodeResource {
                    selector: req.node_selector.clone(),
                })?;

            let node_status = response
                .json::<BottlerocketNodeStatus>()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::UpdateBottlerocketNodeResource {
                    selector: req.node_selector.clone(),
                })?;

            Ok(node_status)
        })
        .await
    }

    #[instrument]
    async fn drain_and_cordon_node(
        &self,
        _req: DrainAndCordonBottlerocketNodeRequest,
    ) -> Result<()> {
        todo!()
    }

    #[instrument]
    async fn uncordon_node(&self, _req: UncordonBottlerocketNodeRequest) -> Result<()> {
        todo!()
    }
}
