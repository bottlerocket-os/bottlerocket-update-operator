/*!
apiclient is a client for interacting with the Bottlerocket Update API.
Brupop volume mounts apiclient binary into the agent container.

Bottlerocket Update API: https://github.com/bottlerocket-os/bottlerocket/tree/develop/sources/updater
Bottlerocket apiclient: https://github.com/bottlerocket-os/bottlerocket/tree/develop/sources/api/apiclient
*/

use semver::Version;
use serde::Deserialize;
use snafu::{ensure, ResultExt};
use std::process::{Command, Output};
use tokio::time::{sleep, Duration};

const API_CLIENT_BIN: &str = "apiclient";
const UPDATES_STATUS_URI: &str = "/updates/status";
const OS_URI: &str = "/os";
const REFRESH_UPDATES_URI: &str = "/actions/refresh-updates";
const PREPARE_UPDATES_URI: &str = "/actions/prepare-update";
const ACTIVATE_UPDATES_URI: &str = "/actions/activate-update";
const REBOOT_URI: &str = "/actions/reboot";
const MAX_ATTEMPTS: i8 = 5;
const UPDATE_API_SLEEP_DURATION: Duration = Duration::from_millis(10000);
const UPDATE_API_BUSY_STATUSCODE: &str = "423";

/// The module-wide result type.
pub type Result<T> = std::result::Result<T, apiclient_error::Error>;

/// UpdateState represents four states during system update process
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize)]
pub enum UpdateState {
    // Idle: Unknow
    Idle,
    // Available: available versions system can update to
    Available,
    // Staged: processing system update (refresh, prepare, activate)
    Staged,
    // Ready: successfully complete update commands, ready to reboot
    Ready,
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
enum CommandStatus {
    Success,
    Failed,
    Unknown,
}

#[derive(Debug, Deserialize)]
pub struct StagedImage {
    image: Option<UpdateImage>,
    next_to_boot: bool,
}

#[derive(Debug, Deserialize)]
pub struct CommandResult {
    cmd_type: UpdateCommand,
    cmd_status: CommandStatus,
    timestamp: String,
    exit_status: u32,
    stderr: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateImage {
    arch: String,
    version: String,
    variant: String,
}

#[derive(Debug, Deserialize)]
pub struct OsInfo {
    pub version_id: Version,
}

#[derive(Debug, Deserialize)]
pub struct UpdateStatus {
    update_state: UpdateState,
    available_updates: Vec<Version>,
    chosen_update: Option<UpdateImage>,
    active_partition: Option<StagedImage>,
    staging_partition: Option<StagedImage>,
    most_recent_command: CommandResult,
}

fn get_raw_args(mut args: Vec<String>) -> Vec<String> {
    let mut subcommand_args = vec!["raw".to_string(), "-u".to_string()];
    subcommand_args.append(&mut args);

    return subcommand_args;
}

/// Extract error statuscode from stderr string
/// Error Example:
/// "Failed POST request to '/actions/refresh-updates': Status 423 when POSTing /actions/refresh-updates: Update lock held\n"
fn extract_status_code_from_error(error: &str) -> &str {
    let error_content_split_by_status: Vec<&str> = error.split("Status").collect();
    let error_content_split_by_whitespace: Vec<&str> = error_content_split_by_status[1]
        .split_whitespace()
        .collect();
    let error_statuscode = error_content_split_by_whitespace[0];

    error_statuscode
}

/// Apiclient binary has been volume mounted into the agent container, so agent is able to
/// invoke `/bin apiclient` to interact with the Bottlerocket Update API.
/// This function helps to invoke apiclient raw command.
async fn invoke_apiclient(args: Vec<String>) -> Result<Output> {
    let mut attempts: i8 = 0;
    // Retry up to 5 times in case the Update API is busy; Waiting 10 seconds between each attempt.
    while attempts < MAX_ATTEMPTS {
        let output = Command::new(API_CLIENT_BIN)
            .args(&args)
            .output()
            .context(apiclient_error::ApiClientRawCommand { args: args.clone() })?;

        if output.status.success() {
            return Ok(output);
        }
        let error_content = String::from_utf8_lossy(&output.stderr).to_string();
        let error_statuscode = extract_status_code_from_error(&error_content);

        match error_statuscode {
            UPDATE_API_BUSY_STATUSCODE => {
                log::info!(
                    "API server busy, retrying in {:?} seconds ...",
                    UPDATE_API_SLEEP_DURATION
                );
                // Retry after ten seconds if we get a 423 Locked response (update API busy)
                sleep(UPDATE_API_SLEEP_DURATION).await;
                attempts += 1;
            }
            _ => {
                // API response was a non-transient error, bail out
                return apiclient_error::BadHttpResponse {
                    statuscode: error_statuscode,
                }
                .fail();
            }
        };
    }
    // Update API is currently unavailable, bail out
    Err(apiclient_error::Error::UpdateApiUnavailable { args })
}

pub async fn get_update_status() -> Result<UpdateStatus> {
    // Refresh list of updates and check if there are any available
    refresh_updates().await?;

    let raw_args = vec![UPDATES_STATUS_URI.to_string()];

    let update_status_output = invoke_apiclient(get_raw_args(raw_args)).await?;

    let update_status_string = String::from_utf8_lossy(&update_status_output.stdout).to_string();
    let update_status: UpdateStatus = serde_json::from_str(&update_status_string)
        .context(apiclient_error::UpdateStatusContent)?;

    Ok(update_status)
}

/// extract OS info from the local system. For now this is just the version id.
pub async fn get_os_info() -> Result<OsInfo> {
    let raw_args = vec![OS_URI.to_string()];

    let os_info_output = invoke_apiclient(get_raw_args(raw_args)).await?;

    let os_info_content_string = String::from_utf8_lossy(&os_info_output.stdout).to_string();
    let os_info: OsInfo =
        serde_json::from_str(&os_info_content_string).context(apiclient_error::OsContent)?;

    Ok(os_info)
}

pub async fn refresh_updates() -> Result<Output> {
    let raw_args = vec![
        REFRESH_UPDATES_URI.to_string(),
        "-m".to_string(),
        "POST".to_string(),
    ];

    Ok(invoke_apiclient(get_raw_args(raw_args)).await?)
}

pub async fn prepare_update() -> Result<()> {
    let raw_args = vec![
        PREPARE_UPDATES_URI.to_string(),
        "-m".to_string(),
        "POST".to_string(),
    ];

    invoke_apiclient(get_raw_args(raw_args)).await?;

    Ok(())
}

pub async fn activate_update() -> Result<()> {
    let raw_args = vec![
        ACTIVATE_UPDATES_URI.to_string(),
        "-m".to_string(),
        "POST".to_string(),
    ];

    invoke_apiclient(get_raw_args(raw_args)).await?;

    Ok(())
}

pub async fn reboot() -> Result<Output> {
    let raw_args = vec![REBOOT_URI.to_string(), "-m".to_string(), "POST".to_string()];

    Ok(invoke_apiclient(get_raw_args(raw_args)).await?)
}

// List all available versions which current Bottlerocket OS can update to.
pub async fn list_available() -> Result<Vec<Version>> {
    // Refresh list of updates and check if there are any available
    refresh_updates().await?;

    let update_status = get_update_status().await?;

    // Raise error if failed to refresh update or update acton performed out of band
    ensure!(
        update_status.most_recent_command.cmd_type == UpdateCommand::Refresh
            && update_status.most_recent_command.cmd_status == CommandStatus::Success,
        apiclient_error::RefreshUpdate
    );

    Ok(update_status.available_updates)
}

pub async fn prepare() -> Result<()> {
    let update_status = get_update_status().await?;

    ensure!(
        update_status.update_state == UpdateState::Available
            || update_status.update_state == UpdateState::Staged,
        apiclient_error::UpdateStage {
            expect_state: "Available or Staged".to_string(),
            update_state: update_status.update_state,
        },
    );

    // Download the update and apply it to the inactive partition
    prepare_update().await?;

    // Raise error if failed to prepare update or update action performed out of band
    let recent_command = get_update_status().await?.most_recent_command;
    ensure!(
        recent_command.cmd_type == UpdateCommand::Prepare
            || recent_command.cmd_status == CommandStatus::Success,
        apiclient_error::PrepareUpdate
    );

    Ok(())
}

pub async fn update() -> Result<()> {
    let update_status = get_update_status().await?;

    ensure!(
        update_status.update_state == UpdateState::Staged,
        apiclient_error::UpdateStage {
            expect_state: "Staged".to_string(),
            update_state: update_status.update_state,
        }
    );

    // Activate the prepared update
    activate_update().await?;

    // Raise error if failed to activate update or update action performed out of band
    let recent_command = get_update_status().await?.most_recent_command;

    ensure!(
        recent_command.cmd_type == UpdateCommand::Activate
            || recent_command.cmd_status == CommandStatus::Success,
        apiclient_error::Update
    );

    Ok(())
}

// Reboot the host into the activated update
pub async fn boot_update() -> Result<Output> {
    let update_status = get_update_status().await?;

    ensure!(
        update_status.update_state == UpdateState::Ready,
        apiclient_error::UpdateStage {
            expect_state: "Ready".to_string(),
            update_state: update_status.update_state,
        }
    );

    Ok(reboot().await?)
}

pub mod apiclient_error {

    use crate::apiclient::UpdateState;
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub")]
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

        #[snafu(display("failed to refresh updates or update action performed out of band"))]
        RefreshUpdate {},

        #[snafu(display("failed to prepare update or update action performed out of band"))]
        PrepareUpdate {},

        #[snafu(display("failed to activate update or update action performed out of band"))]
        Update {},

        #[snafu(display(
        "unexpected update state: {:?}, expecting state to be {}. update action performed out of band?",
         update_state, expect_state
    ))]
        UpdateStage {
            expect_state: String,
            update_state: UpdateState,
        },
        #[snafu(display("bad http response, status code: {}", statuscode))]
        BadHttpResponse { statuscode: String },

        #[snafu(display("Unable to process command apiclient {}: update API unavailable: retries exhausted", args.join(" ")))]
        UpdateApiUnavailable { args: Vec<String> },

        #[snafu(display("Unable to parse version information: '{}'", source))]
        VersionParseError { source: semver::Error },
    }
}
