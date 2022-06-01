use super::error::{self, Result};
use crate::{
    constants::{
        HEADER_BRUPOP_K8S_AUTH_TOKEN, HEADER_BRUPOP_NODE_NAME, HEADER_BRUPOP_NODE_UID,
        NODE_CORDON_AND_DRAIN_ENDPOINT, NODE_RESOURCE_ENDPOINT, NODE_UNCORDON_ENDPOINT,
    },
    CordonAndDrainBottlerocketShadowRequest, CreateBottlerocketShadowRequest,
    UncordonBottlerocketShadowRequest, UpdateBottlerocketShadowRequest,
};
use models::{
    constants::{
        APISERVER_SERVICE_NAME, APISERVER_SERVICE_PORT, NAMESPACE, PUBLIC_KEY_NAME,
        TLS_KEY_MOUNT_PATH,
    },
    node::{BottlerocketShadow, BottlerocketShadowSelector, BottlerocketShadowStatus},
};

use async_trait::async_trait;
use snafu::ResultExt;
use std::fs;
use std::io::Read;
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
    async fn create_bottlerocket_shadow(
        &self,
        req: CreateBottlerocketShadowRequest,
    ) -> Result<BottlerocketShadow>;
    async fn update_bottlerocket_shadow(
        &self,
        req: UpdateBottlerocketShadowRequest,
    ) -> Result<BottlerocketShadowStatus>;
    async fn cordon_and_drain_node(
        &self,
        req: CordonAndDrainBottlerocketShadowRequest,
    ) -> Result<()>;
    async fn uncordon_node(&self, req: UncordonBottlerocketShadowRequest) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct K8SAPIServerClient {
    k8s_projected_token_path: String,
}

impl K8SAPIServerClient {
    pub fn new(k8s_projected_token_path: String) -> Self {
        Self {
            k8s_projected_token_path,
        }
    }

    /// Reads a projected auth token from the configured path.
    fn auth_token(&self) -> Result<String> {
        fs::read_to_string(&self.k8s_projected_token_path)
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::IOError)
    }

    /// Protocol scheme for communicating with the server.
    pub fn scheme() -> String {
        "https".to_string()
    }

    /// Returns the domain on which the server can be reached.
    pub fn server_domain() -> String {
        format!(
            "{}.{}.svc.cluster.local:{}",
            APISERVER_SERVICE_NAME, NAMESPACE, APISERVER_SERVICE_PORT
        )
    }

    fn add_common_request_headers(
        &self,
        req: reqwest::RequestBuilder,
        node_selector: &BottlerocketShadowSelector,
    ) -> Result<reqwest::RequestBuilder> {
        Ok(req
            .header(HEADER_BRUPOP_NODE_UID, &node_selector.node_uid)
            .header(HEADER_BRUPOP_NODE_NAME, &node_selector.node_name)
            .header(HEADER_BRUPOP_K8S_AUTH_TOKEN, &self.auth_token()?))
    }

    /// Returns the https client configured to use self-signed certificate
    fn https_client() -> Result<reqwest::Client> {
        let mut buf = Vec::new();

        let public_key_path = format!("{}/{}", TLS_KEY_MOUNT_PATH, PUBLIC_KEY_NAME);
        std::fs::File::open(public_key_path)
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::IOError)?
            .read_to_end(&mut buf)
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            .context(error::IOError)?;

        let cert = reqwest::Certificate::from_pem(&buf).context(error::CreateClientError)?;

        let client = reqwest::Client::builder()
            .add_root_certificate(cert)
            .connection_verbose(true)
            .build()
            .context(error::CreateClientError)?;
        Ok(client)
    }
}

#[async_trait]
impl APIServerClient for K8SAPIServerClient {
    #[instrument]
    async fn create_bottlerocket_shadow(
        &self,
        req: CreateBottlerocketShadowRequest,
    ) -> Result<BottlerocketShadow> {
        Retry::spawn(retry_strategy(), || async {
            let https_client = Self::https_client()?;

            let request_builder = self.add_common_request_headers(
                https_client.post(format!(
                    "{}://{}{}",
                    Self::scheme(),
                    Self::server_domain(),
                    NODE_RESOURCE_ENDPOINT
                )),
                &req.node_selector,
            )?;

            let response = request_builder
                .json(&req)
                .send()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::CreateBottlerocketShadowResource {
                    selector: req.node_selector.clone(),
                })?;

            let status = response.status();
            if status.is_success() {
                let node = response
                    .json::<BottlerocketShadow>()
                    .await
                    .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                    .context(error::CreateBottlerocketShadowResource {
                        selector: req.node_selector.clone(),
                    })?;
                Ok(node)
            } else {
                Err(Box::new(error::ClientError::ErrorResponse {
                    status_code: status,
                    response: response
                        .text()
                        .await
                        .unwrap_or("<empty response>".to_string()),
                }) as Box<dyn std::error::Error>)
                .context(error::CreateBottlerocketShadowResource {
                    selector: req.node_selector.clone(),
                })
            }
        })
        .await
    }

    #[instrument]
    async fn update_bottlerocket_shadow(
        &self,
        req: UpdateBottlerocketShadowRequest,
    ) -> Result<BottlerocketShadowStatus> {
        Retry::spawn(retry_strategy(), || async {
            let https_client = Self::https_client()?;
            let request_builder = self.add_common_request_headers(
                https_client.put(format!(
                    "{}://{}{}",
                    Self::scheme(),
                    Self::server_domain(),
                    NODE_RESOURCE_ENDPOINT
                )),
                &req.node_selector,
            )?;

            let response = request_builder
                .json(&req.node_status)
                .send()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::UpdateBottlerocketShadowResource {
                    selector: req.node_selector.clone(),
                })?;

            let status = response.status();
            if status.is_success() {
                let node_status = response
                    .json::<BottlerocketShadowStatus>()
                    .await
                    .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                    .context(error::UpdateBottlerocketShadowResource {
                        selector: req.node_selector.clone(),
                    })?;

                Ok(node_status)
            } else {
                Err(Box::new(error::ClientError::ErrorResponse {
                    status_code: status,
                    response: response
                        .text()
                        .await
                        .unwrap_or("<empty response>".to_string()),
                }) as Box<dyn std::error::Error>)
                .context(error::UpdateBottlerocketShadowResource {
                    selector: req.node_selector.clone(),
                })
            }
        })
        .await
    }

    #[instrument]
    async fn cordon_and_drain_node(
        &self,
        req: CordonAndDrainBottlerocketShadowRequest,
    ) -> Result<()> {
        Retry::spawn(retry_strategy(), || async {
            let https_client = Self::https_client()?;
            let request_builder = self.add_common_request_headers(
                https_client.post(format!(
                    "{}://{}{}",
                    Self::scheme(),
                    Self::server_domain(),
                    NODE_CORDON_AND_DRAIN_ENDPOINT
                )),
                &req.node_selector,
            )?;

            let response = request_builder
                .send()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::CordonAndDrainNodeResource {
                    selector: req.node_selector.clone(),
                })?;

            let status = response.status();
            if status.is_success() {
                Ok(())
            } else {
                Err(Box::new(error::ClientError::ErrorResponse {
                    status_code: status,
                    response: response
                        .text()
                        .await
                        .unwrap_or("<empty response>".to_string()),
                }) as Box<dyn std::error::Error>)
                .context(error::CordonAndDrainNodeResource {
                    selector: req.node_selector.clone(),
                })
            }
        })
        .await
    }

    #[instrument]
    async fn uncordon_node(&self, req: UncordonBottlerocketShadowRequest) -> Result<()> {
        Retry::spawn(retry_strategy(), || async {
            let https_client = Self::https_client()?;
            let request_builder = self.add_common_request_headers(
                https_client.post(format!(
                    "{}://{}{}",
                    Self::scheme(),
                    Self::server_domain(),
                    NODE_UNCORDON_ENDPOINT
                )),
                &req.node_selector,
            )?;

            let response = request_builder
                .send()
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
                .context(error::CordonAndDrainNodeResource {
                    selector: req.node_selector.clone(),
                })?;

            let status = response.status();
            if status.is_success() {
                Ok(())
            } else {
                Err(Box::new(error::ClientError::ErrorResponse {
                    status_code: status,
                    response: response
                        .text()
                        .await
                        .unwrap_or("<empty response>".to_string()),
                }) as Box<dyn std::error::Error>)
                .context(error::CordonAndDrainNodeResource {
                    selector: req.node_selector.clone(),
                })
            }
        })
        .await
    }
}
