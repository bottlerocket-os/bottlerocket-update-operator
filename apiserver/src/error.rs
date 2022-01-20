use models::node::BottlerocketNodeError;

use actix_web::error::ResponseError;
use snafu::Snafu;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// The crate-wide error type.
#[derive(Debug, Snafu)]
#[snafu(visibility = "pub")]
pub enum Error {
    #[snafu(display("Unable to create client: '{}'", source))]
    ClientCreate { source: kube::Error },

    #[snafu(display("Unable to parse HTTP header. Missing '{}'", missing_header))]
    HTTPHeaderParse { missing_header: &'static str },

    #[snafu(display("Error creating BottlerocketNode: '{}'", source))]
    BottlerocketNodeCreate { source: BottlerocketNodeError },

    #[snafu(display("Error patching BottlerocketNode: '{}'", source))]
    BottlerocketNodeUpdate { source: BottlerocketNodeError },

    #[snafu(display("Error running HTTP server: '{}'", source))]
    HttpServerError { source: std::io::Error },

    #[snafu(display("Error configuring tracing: '{}'", source))]
    TracingConfiguration {
        source: tracing::subscriber::SetGlobalDefaultError,
    },

    #[snafu(display("The Kubernetes WATCH on Pod objects has failed."))]
    KubernetesWatcherFailed {},

    #[snafu(display("Failed to cordon Node: '{}'", source))]
    BottlerocketNodeCordon { source: BottlerocketNodeError },

    #[snafu(display("Failed to drain Node: '{}'", source))]
    BottlerocketNodeDrain { source: BottlerocketNodeError },
}

impl ResponseError for Error {}
