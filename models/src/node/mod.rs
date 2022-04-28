mod client;
mod crd;
mod drain;
mod error;

pub use self::client::*;
pub use self::crd::*;
pub use self::error::Error as BottlerocketShadowError;

use lazy_static::lazy_static;
pub use semver::Version;

lazy_static! {
    // Regex gathered from semver.org as the recommended semver validation regex.
    static ref SEMVER_RE: regex::Regex = regex::Regex::new(
        concat!(
            r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)",
            r"(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?",
            r"(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$"
        ))
        .expect("Invalid regex literal.");
}

#[cfg(feature = "mockall")]
pub use self::client::MockBottlerocketShadowClient;

// We can't use these consts inside macros, but we do provide constants for use in generating kubernetes objects.
pub const K8S_NODE_KIND: &str = "BottlerocketShadow";
pub const K8S_NODE_PLURAL: &str = "bottlerocketshadows";
pub const K8S_NODE_STATUS: &str = "bottlerocketshadows/status";
pub const K8S_NODE_SHORTNAME: &str = "brs";
