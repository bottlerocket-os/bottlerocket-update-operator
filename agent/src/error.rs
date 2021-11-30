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

    #[snafu(display(
        "Unable to fetch the Kubernetes custom resource associated with this node: '{}'",
        source
    ))]
    FetchCustomResource { source: kube::Error },

    #[snafu(display("Unable to get associated node name: '{}'", source))]
    GetNodeName { source: std::env::VarError },

    #[snafu(display(
        "Unable to fetch the Kubernetes node associated with this pod: '{}'",
        source
    ))]
    FetchNode { source: kube::Error },

    #[snafu(display("Unable to get Node uid because of missing Node `uid` value"))]
    MissingNodeUid {},

    #[snafu(display("Fail to get Node selector value: Node selector value is None"))]
    NodeSelectorIsNone {},

    #[snafu(display("Unable to get Bottlerocket Node {}: '{}'", node_name, source))]
    BottlerocketNodeNotExist {
        node_name: String,
        source: kube::Error,
    },

    #[snafu(display(
        "Unable to get Bottlerocket node 'status' because of missing 'status' value"
    ))]
    MissingBottlerocketNodeStatus,

    #[snafu(display("Unable to get Bottlerocket node 'spec' because of missing 'spec' value"))]
    MissingBottlerocketNodeSpec,

    #[snafu(display("Unable to gather system version metadata: '{}'", source))]
    BottlerocketNodeStatusVersion { source: apiclient_error::Error },

    #[snafu(display("Unable to gather system available versions metadata: '{}'", source))]
    BottlerocketNodeStatusAvailableVersions { source: apiclient_error::Error },

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
}
