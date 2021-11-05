use agent::agentclient::BrupopAgent;
use agent::error::{self, Result};
use apiserver::client::K8SAPIServerClient;
use models::agent::{AGENT_TOKEN_PATH, TOKEN_PROJECTION_MOUNT_PATH};

use snafu::{OptionExt, ResultExt};

use std::env;
use std::fs;
use std::path::Path;

const TERMINATION_LOG: &str = "/dev/termination-log";
#[tokio::main]
async fn main() {
    let termination_log = env::var("TERMINATION_LOG").unwrap_or(TERMINATION_LOG.to_string());

    match run_agent().await {
        Err(error) => {
            fs::write(&termination_log, format!("{}", error))
                .expect("Could not write k8s termination log.");
        }
        Ok(()) => {}
    }
}

async fn run_agent() -> Result<()> {
    env_logger::init();

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreate)?;

    // Configure our brupop apiserver client to use the auth token mounted to our Pod.
    let token_path = Path::new(TOKEN_PROJECTION_MOUNT_PATH).join(AGENT_TOKEN_PATH);
    let token_path = token_path.to_str().context(error::Assertion {
        message: "Token path (defined in models/agent.rs) is not valid unicode.",
    })?;
    let apiserver_client = K8SAPIServerClient::new(token_path.to_string());

    let mut agent = BrupopAgent::new(k8s_client, apiserver_client);

    // Create a bottlerocketnode (custom resource) if associated bottlerocketnode does not exist
    if !agent.check_node_custom_resource_exists().await? {
        agent.create_metadata_custom_resource().await?;
    }

    // Initialize bottlerocketnode (custom resource) `status` if associated bottlerocketnode does not have `status`
    if !agent.check_custom_resource_status_exists().await? {
        agent.initialize_metadata_custom_resource().await?;
    }

    agent.run().await
}
