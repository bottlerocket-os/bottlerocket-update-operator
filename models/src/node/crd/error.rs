use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display(
        "BottlerocketShadow object ('{}') is missing a reference to the owning Node.",
        name
    ))]
    MissingOwnerReference { name: String },

    #[snafu(display(
        "BottlerocketShadow object must have valid rfc3339 timestamp: '{}'",
        source
    ))]
    TimestampFormat { source: chrono::ParseError },

    #[snafu(display(
        "IO error occurred while attempting to use APIServerClient: '{}'",
        source
    ))]
    IOError { source: Box<dyn std::error::Error> },
}
