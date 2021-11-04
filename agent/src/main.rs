use agent::agentclient::BrupopAgent;
use agent::error::{self, Result};

use snafu::ResultExt;

use std::env;
use std::fs;

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

    let mut agent = BrupopAgent::new(k8s_client);

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
