use models::node::BottlerocketNodeError;

use snafu::Snafu;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// The crate-wide error type.
#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum Error {
    #[snafu(display("Unable to create client: '{}'", source))]
    ClientCreate { source: kube::Error },

    #[snafu(display("Error configuring tracing: '{}'", source))]
    TracingConfiguration {
        source: tracing::subscriber::SetGlobalDefaultError,
    },

    #[snafu(display("Attempted to process node without set status: '{}'", node_name))]
    NodeWithoutStatus { node_name: String },

    #[snafu(display(
        "Cannot determine the next node spec based on the current node state: '{}'",
        source
    ))]
    NodeSpecCannotBeDetermined { source: BottlerocketNodeError },

    #[snafu(display("Failed to update node spec via kubernetes API: '{}'", source))]
    UpdateNodeSpec { source: BottlerocketNodeError },

    #[snafu(display("Could not determine selector for node: '{}'", source))]
    NodeSelectorCreation { source: BottlerocketNodeError },
}
