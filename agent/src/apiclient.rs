/*!
apiclient is a client for interacting with the Bottlerocket Update API.
Brupop volume mounts apiclient binary into the agent container.

Bottlerocket Update API: https://github.com/bottlerocket-os/bottlerocket/tree/develop/sources/updater
Bottlerocket apiclient: https://github.com/bottlerocket-os/bottlerocket/tree/develop/sources/api/apiclient
*/

use self::api::{CommandStatus, UpdateCommand, UpdateState};
pub use self::api::{OsInfo, UpdateImage};
use snafu::ensure;
use std::process::Output;

/// The module-wide result type.
pub type Result<T> = std::result::Result<T, apiclient_error::Error>;

/// extract OS info from the local system.
pub async fn get_os_info() -> Result<OsInfo> {
    api::get_os_info().await
}

// get chosen update which contains latest Bottlerocket OS can update to.
pub async fn get_chosen_update() -> Result<Option<UpdateImage>> {
    api::refresh_updates().await?;

    let update_status = api::get_update_status().await?;

    ensure!(
        update_status.most_recent_command.cmd_type == UpdateCommand::Refresh
            && update_status.most_recent_command.cmd_status == CommandStatus::Success,
        apiclient_error::RefreshUpdateSnafu
    );

    Ok(update_status.chosen_update)
}

pub async fn prepare_update() -> Result<()> {
    let update_status = api::get_update_status().await?;

    ensure!(
        update_status.update_state == UpdateState::Available
            || update_status.update_state == UpdateState::Staged,
        apiclient_error::UpdateStageSnafu {
            expect_state: "Available or Staged".to_string(),
            update_state: update_status.update_state,
        },
    );

    // Download the update and apply it to the inactive partition
    api::prepare_update().await?;

    // Raise error if failed to prepare update or update action performed out of band
    let recent_command = api::get_update_status().await?.most_recent_command;
    ensure!(
        recent_command.cmd_type == UpdateCommand::Prepare
            || recent_command.cmd_status == CommandStatus::Success,
        apiclient_error::PrepareUpdateSnafu
    );

    Ok(())
}

pub async fn activate_update() -> Result<()> {
    let update_status = api::get_update_status().await?;

    ensure!(
        update_status.update_state == UpdateState::Staged,
        apiclient_error::UpdateStageSnafu {
            expect_state: "Staged".to_string(),
            update_state: update_status.update_state,
        }
    );

    // Activate the prepared update
    api::activate_update().await?;

    // Raise error if failed to activate update or update action performed out of band
    let recent_command = api::get_update_status().await?.most_recent_command;

    ensure!(
        recent_command.cmd_type == UpdateCommand::Activate
            || recent_command.cmd_status == CommandStatus::Success,
        apiclient_error::UpdateSnafu
    );

    Ok(())
}

// Reboot the host into the activated update
pub async fn boot_into_update() -> Result<Output> {
    let update_status = api::get_update_status().await?;

    ensure!(
        update_status.update_state == UpdateState::Ready,
        apiclient_error::UpdateStageSnafu {
            expect_state: "Ready".to_string(),
            update_state: update_status.update_state,
        }
    );

    api::reboot().await
}

pub(super) mod api {
    //! Low-level Bottlerocket update API interactions
    use super::{apiclient_error, Result};
    use governor::{
        clock::DefaultClock,
        middleware::NoOpMiddleware,
        state::{InMemoryState, NotKeyed},
        Quota, RateLimiter,
    };
    use lazy_static::lazy_static;
    use nonzero_ext::nonzero;
    use semver::Version;
    use serde::Deserialize;
    use snafu::ResultExt;
    use std::process::{Command, Output};
    use tokio::time::Duration;
    use tokio_retry::{
        strategy::{jitter, ExponentialBackoff},
        Retry,
    };
    use tracing::{event, instrument, Level};

    const API_CLIENT_BIN: &str = "apiclient";
    const UPDATE_API_BUSY_STATUSCODE: &str = "423";
    const ACTIVATE_UPDATES_URI: &str = "/actions/activate-update";
    const OS_URI: &str = "/os";
    const PREPARE_UPDATES_URI: &str = "/actions/prepare-update";
    const REBOOT_URI: &str = "/actions/reboot";
    const REFRESH_UPDATES_URI: &str = "/actions/refresh-updates";
    const UPDATES_STATUS_URI: &str = "/updates/status";

    type SimpleRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

    lazy_static! {
        static ref UPDATE_API_RATE_LIMITER: SimpleRateLimiter = RateLimiter::direct(
            Quota::with_period(Duration::from_secs(10))
                .unwrap()
                .allow_burst(nonzero!(2u32))
        );
    }

    pub(super) fn get_raw_args(mut args: Vec<String>) -> Vec<String> {
        let mut subcommand_args = vec!["raw".to_string(), "-u".to_string()];
        subcommand_args.append(&mut args);

        subcommand_args
    }

    #[derive(Debug, Deserialize)]
    pub struct UpdateStatus {
        pub update_state: UpdateState,
        #[serde(rename = "available_updates")]
        pub _available_updates: Vec<Version>,
        pub chosen_update: Option<UpdateImage>,
        #[serde(rename = "active_partition")]
        pub _active_partition: Option<StagedImage>,
        #[serde(rename = "staging_partition")]
        pub _staging_partition: Option<StagedImage>,
        pub most_recent_command: CommandResult,
    }

    /// UpdateState represents four states during system update process
    #[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize)]
    pub enum UpdateState {
        // Idle: Unknown
        Idle,
        // Available: available versions system can update to
        Available,
        // Staged: processing system update (refresh, prepare, activate)
        Staged,
        // Ready: successfully complete update commands, ready to reboot
        Ready,
    }

    #[derive(Debug, Deserialize)]
    pub struct UpdateImage {
        #[serde(rename = "arch")]
        pub _arch: String,
        pub version: Version,
        #[serde(rename = "variant")]
        pub _variant: String,
    }

    /// UpdateCommand represents three commands to update system
    #[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum UpdateCommand {
        // Refresh: refresh the list of available updates
        Refresh,
        // Prepare: request that the update be downloaded and applied to disk
        Prepare,
        // Activate: proceed to "activate" the update
        Activate,
    }

    /// CommandStatus represents three status after running update command
    #[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize)]
    pub enum CommandStatus {
        Success,
        Failed,
        Unknown,
    }

    #[derive(Debug, Deserialize)]
    pub struct StagedImage {
        #[serde(rename = "image")]
        _image: Option<UpdateImage>,
        #[serde(rename = "next_to_boot")]
        _next_to_boot: bool,
    }

    #[derive(Debug, Deserialize)]
    pub struct CommandResult {
        pub cmd_type: UpdateCommand,
        pub cmd_status: CommandStatus,
        #[serde(rename = "timestamp")]
        _timestamp: String,
        #[serde(rename = "exit_status")]
        _exit_status: u32,
        #[serde(rename = "stderr")]
        _stderr: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct OsInfo {
        pub version_id: Version,
    }

    /// Extract error statuscode from stderr string
    /// Error Example:
    /// "Failed POST request to '/actions/refresh-updates': Status 423 when POSTing /actions/refresh-updates: Update lock held\n"
    fn extract_status_code_from_error(error: &str) -> &str {
        let error_content_split_by_status: Vec<&str> = error.split("Status").collect();
        let error_content_split_by_whitespace: Vec<&str> = error_content_split_by_status[1]
            .split_whitespace()
            .collect();
        error_content_split_by_whitespace[0]
    }

    /// Wait time between invoking the Bottlerocket API
    const RETRY_BASE_DELAY: Duration = Duration::from_secs(10);
    const RETRY_MAX_DELAY: Duration = Duration::from_secs(60);
    /// Number of retries while invoking the Bottlerocket API
    const NUM_RETRIES: usize = 5;

    /// Retry strategy for invoking the Bottlerocket API.
    /// Retries to the bottlerocket API occur on a fixed interval with jitter.
    fn retry_strategy() -> impl Iterator<Item = Duration> {
        ExponentialBackoff::from_millis(RETRY_BASE_DELAY.as_millis() as u64)
            .max_delay(RETRY_MAX_DELAY)
            .map(jitter)
            .take(NUM_RETRIES)
    }

    /// Apiclient binary has been volume mounted into the agent container, so agent is able to
    /// invoke `/bin apiclient` to interact with the Bottlerocket Update API.
    /// This function helps to invoke apiclient raw command.
    #[instrument(err, skip(rate_limiter))]
    pub(super) async fn invoke_apiclient(
        args: Vec<String>,
        rate_limiter: Option<&SimpleRateLimiter>,
    ) -> Result<Output> {
        Retry::spawn(retry_strategy(), || async {
            event!(Level::DEBUG, "Invoking apiclient: {:?}", args);
            if let Some(rate_limiter) = rate_limiter {
                if let Err(e) = rate_limiter.check() {
                    event!(
                        Level::DEBUG,
                        "apiclient rate limited until {:?}",
                        e.earliest_possible()
                    );
                    rate_limiter.until_ready().await;
                }
            }
            let output = Command::new(API_CLIENT_BIN)
                .args(&args)
                .output()
                .context(apiclient_error::ApiClientRawCommandSnafu { args: args.clone() })?;

            if output.status.success() {
                Ok(output)
            } else {
                // Return value `exit status` is Option. When the value has `some` value, we need extract error info from stderr and handle those errors.
                // Otherwise, on Unix, this will return `None` if the process was terminated by a signal. Signal termination is not considered a success.
                // Apiclient `Reboot` command will send signal to terminate the process, so we have to consider this situation and have extra logic to recognize
                // return value `None` as success and terminate the process properly.
                match output.status.code() {
                    // when return value has `some` code, this part will handle those errors properly.
                    Some(_code) => {
                        let error_content = String::from_utf8_lossy(&output.stderr).to_string();
                        let error_statuscode = extract_status_code_from_error(&error_content);

                        match error_statuscode {
                            UPDATE_API_BUSY_STATUSCODE => {
                                event!(
                                    Level::DEBUG,
                                    "The lock for the update API is held by another process ..."
                                );
                                apiclient_error::UpdateApiUnavailableSnafu { args: args.clone() }
                                    .fail()
                            }
                            _ => {
                                // API response was a non-transient error, bail out
                                apiclient_error::BadHttpResponseSnafu {
                                    args: args.clone(),
                                    error_content: &error_content,
                                    statuscode: error_statuscode,
                                }
                                .fail()
                            }
                        }
                    }
                    // when it returns `None`, this part will treat it as success and then gracefully exit brupop agent.
                    _ => {
                        event!(
                            Level::INFO,
                            "Bottlerocket node is terminated by reboot signal"
                        );
                        std::process::exit(0)
                    }
                }
            }
        })
        .await
    }

    #[instrument]
    pub(super) async fn refresh_updates() -> Result<Output> {
        let raw_args = vec![
            REFRESH_UPDATES_URI.to_string(),
            "-m".to_string(),
            "POST".to_string(),
        ];

        invoke_apiclient(get_raw_args(raw_args), Some(&UPDATE_API_RATE_LIMITER)).await
    }

    #[instrument]
    pub(super) async fn prepare_update() -> Result<()> {
        let raw_args = vec![
            PREPARE_UPDATES_URI.to_string(),
            "-m".to_string(),
            "POST".to_string(),
        ];

        invoke_apiclient(get_raw_args(raw_args), Some(&UPDATE_API_RATE_LIMITER)).await?;

        Ok(())
    }

    #[instrument]
    pub(super) async fn activate_update() -> Result<()> {
        let raw_args = vec![
            ACTIVATE_UPDATES_URI.to_string(),
            "-m".to_string(),
            "POST".to_string(),
        ];

        invoke_apiclient(get_raw_args(raw_args), Some(&UPDATE_API_RATE_LIMITER)).await?;

        Ok(())
    }

    #[instrument]
    pub(super) async fn get_update_status() -> Result<UpdateStatus> {
        let raw_args = vec![UPDATES_STATUS_URI.to_string()];
        let update_status_output =
            invoke_apiclient(get_raw_args(raw_args), Some(&UPDATE_API_RATE_LIMITER)).await?;

        let update_status_string =
            String::from_utf8_lossy(&update_status_output.stdout).to_string();
        let update_status: UpdateStatus = serde_json::from_str(&update_status_string)
            .context(apiclient_error::UpdateStatusContentSnafu)?;

        Ok(update_status)
    }

    #[instrument]
    pub(super) async fn reboot() -> Result<Output> {
        let raw_args = vec![REBOOT_URI.to_string(), "-m".to_string(), "POST".to_string()];

        invoke_apiclient(get_raw_args(raw_args), None).await
    }

    #[instrument]
    pub(super) async fn get_os_info() -> Result<OsInfo> {
        let raw_args = vec![OS_URI.to_string()];

        let os_info_output = invoke_apiclient(get_raw_args(raw_args), None).await?;

        let os_info_content_string = String::from_utf8_lossy(&os_info_output.stdout).to_string();
        let os_info: OsInfo = serde_json::from_str(&os_info_content_string)
            .context(apiclient_error::OsContentSnafu)?;

        Ok(os_info)
    }
}

pub mod apiclient_error {
    use crate::apiclient::UpdateState;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Failed to run apiclient command apiclient {:?}: {}", args.join(" "), source))]
        ApiClientRawCommand {
            args: Vec<String>,
            source: std::io::Error,
        },

        #[snafu(display("Failed to deserialize Os info: {}", source))]
        OsContent { source: serde_json::Error },

        #[snafu(display("Failed to deserialize update status: {}", source))]
        UpdateStatusContent { source: serde_json::Error },

        #[snafu(display("Failed to refresh updates or update action performed out of band"))]
        RefreshUpdate {},

        #[snafu(display("Failed to prepare update or update action performed out of band"))]
        PrepareUpdate {},

        #[snafu(display("Failed to activate update or update action performed out of band"))]
        Update {},

        #[snafu(display(
        "Unexpected update state: {:?}, expecting state to be {}. Update action performed out of band?",
         update_state, expect_state
    ))]
        UpdateStage {
            expect_state: String,
            update_state: UpdateState,
        },
        #[snafu(display("Bad http response when running command {} due to status code {}. Error output: {}", args.join(" "), statuscode, error_content))]
        BadHttpResponse {
            args: Vec<String>,
            error_content: String,
            statuscode: String,
        },

        #[snafu(display("Unable to process command apiclient {}: The lock for the update API is held by another process. Retries exhausted.", args.join(" ")))]
        UpdateApiUnavailable { args: Vec<String> },

        #[snafu(display("Unable to parse version information: '{}'", source))]
        VersionParseError { source: semver::Error },
    }
}
