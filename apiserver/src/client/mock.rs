/// This module contains client implementations that are useful for testing purposes.
use crate::client::prelude::*;
use async_trait::async_trait;
use mockall::{mock, predicate::*};
use models::node::{BottlerocketShadow, BottlerocketShadowStatus};

type Result<T> = std::result::Result<T, ClientError>;

mock! {
    /// A Mock APIServerClient for use in tests.
    #[derive(Debug)]
    pub APIServerClient {}
    #[async_trait]
    impl APIServerClient for APIServerClient {
        async fn create_bottlerocket_shadow(
            &self,
            req: CreateBottlerocketShadowRequest,
        ) -> Result<BottlerocketShadow>;
        async fn update_bottlerocket_shadow(
            &self,
            req: UpdateBottlerocketShadowRequest,
        ) -> Result<BottlerocketShadowStatus>;
        async fn cordon_and_drain_node(&self, req: CordonAndDrainBottlerocketShadowRequest)
            -> Result<()>;
        async fn uncordon_node(&self, req: UncordonBottlerocketShadowRequest) -> Result<()>;
        async fn exclude_node_from_lb(&self, req: ExcludeNodeFromLoadBalancerRequest) -> Result<()>;
        async fn remove_node_exclusion_from_lb(&self, req: RemoveNodeExclusionFromLoadBalancerRequest) -> Result<()>;
    }
}
