use crate::apiclient::apiclient_error;
use snafu::Snafu;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// The crate-wide error type.
#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum Error {
    #[snafu(display("Unable to create client: '{}'", source))]
    ClientCreate { source: kube::Error },

    #[snafu(display("Unable to get associated node name: {}", source))]
    GetNodeName { source: std::env::VarError },

    #[snafu(display("Unable to get Node uid because of missing Node `uid` value"))]
    MissingNodeUid {},

    #[snafu(display(
        "Error {} when sending to fetch Bottlerocket Node {}",
        source,
        node_name
    ))]
    UnableFetchBottlerocketNode {
        node_name: String,
        source: kube::Error,
    },
    #[snafu(display(
        "ErrorResponse code '{}' when sending to fetch Bottlerocket Node",
        code
    ))]
    FetchBottlerocketNodeErrorCode { code: u16 },

    #[snafu(display(
        "Unable to get Bottlerocket node 'status' because of missing 'status' value"
    ))]
    MissingBottlerocketNodeStatus,

    #[snafu(display("Unable to gather system version metadata: {}", source))]
    BottlerocketNodeStatusVersion { source: apiclient_error::Error },

    #[snafu(display("Unable to gather system chosen update metadata: '{}'", source))]
    BottlerocketNodeStatusChosenUpdate { source: apiclient_error::Error },

    #[snafu(display(
        "Unable to update the custom resource associated with this node: '{}'",
        source
    ))]
    UpdateBottlerocketNodeResource {
        source: apiserver::client::ClientError,
    },

    #[snafu(display(
        "Unable to create the custom resource associated with this node: '{}'",
        source
    ))]
    CreateBottlerocketNodeResource {
        source: apiserver::client::ClientError,
    },

    #[snafu(display("Unable to drain and cordon this node: '{}'", source))]
    CordonAndDrainNode {
        source: apiserver::client::ClientError,
    },

    #[snafu(display("Unable to uncordon this node: '{}'", source))]
    UncordonNode {
        source: apiserver::client::ClientError,
    },

    #[snafu(display("Unable to take action '{}': '{}'", action, source))]
    UpdateActions {
        action: String,
        source: apiclient_error::Error,
    },

    #[snafu(display(
        "Failed to run command 'uptime -s' to get latest system reboot time: '{}'",
        source
    ))]
    GetUptime { source: std::io::Error },

    #[snafu(display("Unable to convert `{} `to datetime: '{}'", uptime, source))]
    ConvertStringToDatetime {
        uptime: String,
        source: chrono::ParseError,
    },
    #[snafu(display(
        "Unable to convert response of interacting with custom resource to readable text: '{}'",
        source
    ))]
    ConvertResponseToText { source: reqwest::Error },

    // The assertion type lets us return a Result in cases where we would otherwise use `unwrap()` on results that
    // we know cannot be Err. This lets us bubble up to our error handler which writes to the termination log.
    #[snafu(display("Agent failed due to internal assertion issue: '{}'", message))]
    Assertion { message: String },

    #[snafu(display(
        "Unable to fetch {} store: Store unavailable: retries exhausted",
        object
    ))]
    ReflectorUnavailable { object: String },
}
