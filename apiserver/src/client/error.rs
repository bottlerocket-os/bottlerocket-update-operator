use models::node::BottlerocketNodeSelector;

use snafu::Snafu;

/// The client result type.
pub type Result<T> = std::result::Result<T, ClientError>;

/// Error type representing issues using an apiserver client.
#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum ClientError {
    #[snafu(display(
        "API server responded with an error status code {}: '{}'",
        status_code,
        response
    ))]
    ErrorResponse {
        status_code: reqwest::StatusCode,
        response: String,
    },

    #[snafu(display(
        "Unable to create BottlerocketNode ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    CreateBottlerocketNodeResource {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketNodeSelector,
    },
    #[snafu(display(
        "Unable to update BottlerocketNode status ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    UpdateBottlerocketNodeResource {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketNodeSelector,
    },

    #[snafu(display(
        "Unable to drain and cordon Node status ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    CordonAndDrainNodeResource {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketNodeSelector,
    },

    #[snafu(display(
        "Unable to uncordon Node status ({}, {}): '{}'",
        selector.node_name,
        selector.node_uid,
        source
    ))]
    UncordonNodeResource {
        source: Box<dyn std::error::Error>,
        selector: BottlerocketNodeSelector,
    },

    #[snafu(display(
        "IO error occurred while attempting to use APIServerClient: '{}'",
        source
    ))]
    IOError { source: Box<dyn std::error::Error> },
}
