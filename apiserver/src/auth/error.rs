use actix_web::{error::ResponseError, http::StatusCode};
use snafu::Snafu;

/// Errors that can occur while authorizing a request from a brupop agent against a particular Node.
#[derive(Debug, Snafu, Clone)]
#[snafu(visibility(pub(crate)))]
pub enum AuthorizationError {
    // kube::Error does not implement clone, so we pull a string message from it.
    #[snafu(display("Failed to create TokenReview request: '{}'", err_msg))]
    TokenReviewCreate { err_msg: String },

    #[snafu(display("The TokenReview Server returned a review without a status"))]
    TokenReviewMissingStatus {},

    #[snafu(display(
        "The TokenReview Server indicated that an error occurred: '{}'",
        err_msg
    ))]
    TokenReviewServerError { err_msg: String },

    #[snafu(display("The TokenReview Server did not authenticate the provided token."))]
    TokenNotAuthenticated {},

    #[snafu(display("The TokenReview Server does not appear to be audience aware."))]
    TokenReviewServerNotAudienceAware {},

    #[snafu(display("APIServer does not seem to be in the audience of the requester's token",))]
    AudienceMismatch {},

    #[snafu(display(
        "Returned TokenReview status does not contain pod metadata required to authorize",
    ))]
    TokenReviewMissingPodName {},

    #[snafu(display("Could not find reference to Pod owning request token: '{}'", pod_name))]
    NoSuchPod { pod_name: String },

    #[snafu(display(
        "Requesting pod's node ('{}') does not match brn selector ('{}')",
        requesting_node,
        target_node
    ))]
    RequesterTargetMismatch {
        requesting_node: String,
        target_node: String,
    },
}

impl ResponseError for AuthorizationError {
    fn status_code(&self) -> StatusCode {
        match *self {
            Self::TokenReviewCreate { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::TokenReviewServerError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::FORBIDDEN,
        }
    }
}
