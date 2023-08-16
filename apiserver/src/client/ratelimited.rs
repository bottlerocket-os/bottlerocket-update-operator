//! This module defines an ApiServerClient implementation that wraps another and rate-limits API calls.
use crate::client::prelude::*;
use async_trait::async_trait;
use governor::{
    clock::{Clock, DefaultClock, ReasonablyRealtime},
    middleware::NoOpMiddleware,
    state::{DirectStateStore, InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use models::node::{BottlerocketShadow, BottlerocketShadowStatus};
use nonzero_ext::nonzero;
use std::{fmt::Debug, sync::Arc};
use std::{num::NonZeroU32, ops::Deref};
use tracing::{event, Level};

type Result<T> = std::result::Result<T, ClientError>;

#[derive(Debug, Clone)]
pub struct RateLimitedAPIServerClient<WC, S, C, RL>
where
    WC: APIServerClient,
    S: DirectStateStore + Debug,
    C: ReasonablyRealtime + Clock + Debug,
    RL: Deref<Target = RateLimiter<NotKeyed, S, C, NoOpMiddleware<C::Instant>>>
        + Send
        + Sync
        + Debug,
{
    rate_limiter: RL,
    wrapped_client: WC,
}

impl<WC, S, C, RL> RateLimitedAPIServerClient<WC, S, C, RL>
where
    WC: APIServerClient,
    S: DirectStateStore + Debug,
    C: ReasonablyRealtime + Clock + Debug,
    RL: Deref<Target = RateLimiter<NotKeyed, S, C, NoOpMiddleware<C::Instant>>>
        + Send
        + Sync
        + Debug,
{
    pub fn new(wrapped_client: WC, rate_limiter: RL) -> Self {
        Self {
            wrapped_client,
            rate_limiter,
        }
    }

    async fn rate_limit(&self) {
        if let Err(e) = self.rate_limiter.check() {
            event!(
                Level::DEBUG,
                "Rate limited while calling api server for {}.",
                e
            );
            self.rate_limiter.until_ready().await;
        }
    }
}

/// Rate at which request token bucket refills.
const DEFAULT_REQUESTS_PER_MINUTE: NonZeroU32 = nonzero!(4u32);

/// Default rate limiter.
type SimpleRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// Provides a rate-limiter with reasonable default settings.
impl<WC> RateLimitedAPIServerClient<WC, InMemoryState, DefaultClock, Arc<SimpleRateLimiter>>
where
    WC: APIServerClient,
{
    pub fn default(wrapped_client: WC) -> Self {
        let rate_limiter = Arc::new(SimpleRateLimiter::direct(Quota::per_minute(
            DEFAULT_REQUESTS_PER_MINUTE,
        )));
        Self {
            wrapped_client,
            rate_limiter,
        }
    }
}

#[async_trait]
impl<WC, S, C, RL> APIServerClient for RateLimitedAPIServerClient<WC, S, C, RL>
where
    WC: APIServerClient,
    S: DirectStateStore + Sync + Send + Debug,
    C: ReasonablyRealtime + Clock + Sync + Send + Debug,
    RL: Deref<Target = RateLimiter<NotKeyed, S, C, NoOpMiddleware<C::Instant>>>
        + Send
        + Sync
        + Debug,
{
    async fn create_bottlerocket_shadow(
        &self,
        req: CreateBottlerocketShadowRequest,
    ) -> Result<BottlerocketShadow> {
        self.rate_limit().await;
        self.wrapped_client.create_bottlerocket_shadow(req).await
    }

    async fn update_bottlerocket_shadow(
        &self,
        req: UpdateBottlerocketShadowRequest,
    ) -> Result<BottlerocketShadowStatus> {
        self.rate_limit().await;
        self.wrapped_client.update_bottlerocket_shadow(req).await
    }

    async fn cordon_and_drain_node(
        &self,
        req: CordonAndDrainBottlerocketShadowRequest,
    ) -> Result<()> {
        self.rate_limit().await;
        self.wrapped_client.cordon_and_drain_node(req).await
    }

    async fn uncordon_node(&self, req: UncordonBottlerocketShadowRequest) -> Result<()> {
        self.rate_limit().await;
        self.wrapped_client.uncordon_node(req).await
    }

    async fn exclude_node_from_lb(&self, req: ExcludeNodeFromLoadBalancerRequest) -> Result<()> {
        self.rate_limit().await;
        self.wrapped_client.exclude_node_from_lb(req).await
    }

    async fn remove_node_exclusion_from_lb(
        &self,
        req: RemoveNodeExclusionFromLoadBalancerRequest,
    ) -> Result<()> {
        self.rate_limit().await;
        self.wrapped_client.remove_node_exclusion_from_lb(req).await
    }
}
