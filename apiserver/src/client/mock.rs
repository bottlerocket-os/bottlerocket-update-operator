/// This module contains client implementations that are useful for testing purposes.
use super::{error::Result, APIServerClient};
use crate::{
    CreateBottlerocketNodeRequest, DrainAndCordonBottlerocketNodeRequest,
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
        async fn drain_and_cordon_node(&self, req: DrainAndCordonBottlerocketNodeRequest)
            -> Result<()>;
        async fn uncordon_node(&self, req: UncordonBottlerocketNodeRequest) -> Result<()>;
    }
}
