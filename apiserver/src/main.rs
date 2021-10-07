use apiserver::api::{self, APIServerSettings};
use apiserver::error::{self, Result};
use models::node::K8SBottlerocketNodeClient;

use snafu::ResultExt;

#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::init();

    let k8s_client = kube::client::Client::try_default()
        .await
        .context(error::ClientCreate)?;

    let settings = APIServerSettings {
        node_client: K8SBottlerocketNodeClient::new(k8s_client),
        server_port: 8080,
    };

    api::run_server(settings).await
}
