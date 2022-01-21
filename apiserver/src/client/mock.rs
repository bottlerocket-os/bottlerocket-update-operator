/// This module contains client implementations that are useful for testing purposes.
use super::{error::Result, APIServerClient};
use crate::{
    CordonAndDrainBottlerocketNodeRequest, CreateBottlerocketNodeRequest,
    UncordonBottlerocketNodeRequest, UpdateBottlerocketNodeRequest,
};
use models::node::{BottlerocketNode, BottlerocketNodeStatus};

use async_trait::async_trait;

use mockall::{mock, predicate::*};

mock! {
    /// A Mock APIServerClient for use in tests.
    pub APIServerClient {}
    #[async_trait]
    impl APIServerClient for APIServerClient {
        async fn create_bottlerocket_node(
            &self,
            req: CreateBottlerocketNodeRequest,
        ) -> Result<BottlerocketNode>;
        async fn update_bottlerocket_node(
            &self,
            req: UpdateBottlerocketNodeRequest,
        ) -> Result<BottlerocketNodeStatus>;
        async fn cordon_and_drain_node(&self, req: CordonAndDrainBottlerocketNodeRequest)
            -> Result<()>;
        async fn uncordon_node(&self, req: UncordonBottlerocketNodeRequest) -> Result<()>;
    }
}
