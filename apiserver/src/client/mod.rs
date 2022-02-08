pub mod error;
mod webclient;

#[cfg(any(feature = "mockall", test))]
pub mod mock;

pub use error::ClientError;
pub use webclient::{APIServerClient, K8SAPIServerClient};
