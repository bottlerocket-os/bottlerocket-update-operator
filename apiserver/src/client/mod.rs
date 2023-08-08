pub mod error;
mod ratelimited;
mod webclient;

#[cfg(any(feature = "mockall", test))]
pub mod mock;

pub use error::ClientError;
pub use ratelimited::RateLimitedAPIServerClient;
pub use webclient::{APIServerClient, K8SAPIServerClient};

pub mod prelude {
    pub use super::error::ClientError;
    pub use super::APIServerClient;
    pub use crate::{
        CordonAndDrainBottlerocketShadowRequest, CreateBottlerocketShadowRequest,
        ExcludeNodeFromLoadBalancerRequest, RemoveNodeExclusionFromLoadBalancerRequest,
        UncordonBottlerocketShadowRequest, UpdateBottlerocketShadowRequest,
    };
}
