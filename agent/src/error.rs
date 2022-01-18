use crate::agentclient::agentclient_error;
use snafu::Snafu;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// The crate-wide error type.
#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum Error {
    #[snafu(display("Error running agent server: '{}'", source))]
    AgentError { source: agentclient_error::Error },

    // The assertion type lets us return a Result in cases where we would otherwise use `unwrap()` on results that
    // we know cannot be Err. This lets us bubble up to our error handler which writes to the termination log.
    #[snafu(display("Agent failed due to internal assertion issue: '{}'", message))]
    Assertion { message: String },

    #[snafu(display("Unable to create client: '{}'", source))]
    ClientCreate { source: kube::Error },

    #[snafu(display("Unable to get associated node name: {}", source))]
    GetNodeName { source: std::env::VarError },

    #[snafu(display("The Kubernetes WATCH on {} objects has failed.", object))]
    KubernetesWatcherFailed { object: String },

    #[snafu(display("Error configuring tracing: '{}'", source))]
    TracingConfiguration {
        source: tracing::subscriber::SetGlobalDefaultError,
    },
}
