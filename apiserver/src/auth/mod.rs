pub(crate) mod authorizor;
pub(crate) mod error;
pub(crate) mod middleware;

pub use authorizor::{K8STokenAuthorizor, K8STokenReviewer, TokenAuthorizor};
pub use error::AuthorizationError;
pub use middleware::TokenAuthMiddleware;

#[cfg(any(mockall, test))]
pub use authorizor::mock;
