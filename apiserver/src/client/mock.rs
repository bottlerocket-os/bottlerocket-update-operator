/// This module contains client implementations that are useful for testing purposes.
use super::{error::Result, APIServerClient};
use crate::{
    CordonAndDrainBottlerocketShadowRequest, CreateBottlerocketShadowRequest,
    UncordonBottlerocketShadowRequest, UpdateBottlerocketShadowRequest,
};
use models::node::{BottlerocketShadow, BottlerocketShadowStatus};

use async_trait::async_trait;

use mockall::{mock, predicate::*};

mock! {
    /// A Mock APIServerClient for use in tests.
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
    }
}
