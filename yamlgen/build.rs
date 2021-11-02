/*!

The custom resource definitions are modeled as Rust structs. Here we generate
the corresponding k8s yaml files.

!*/

use kube::CustomResourceExt;
use models::{
    agent::{
        agent_cluster_role, agent_cluster_role_binding, agent_daemonset, agent_service_account,
    },
    apiserver::{
        apiserver_cluster_role, apiserver_cluster_role_binding, apiserver_deployment,
        apiserver_service, apiserver_service_account,
    },
    namespace::brupop_namespace,
    node::BottlerocketNode,
};
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

const YAMLGEN_DIR: &str = env!("CARGO_MANIFEST_DIR");
const HEADER: &str = "# This file is generated. Do not edit.\n";

fn main() {
    dotenv::dotenv().ok();
    // Re-run this build script if the model changes.
    println!("cargo:rerun-if-changed=../models/src");
    // Re-run the yaml generation if these variables change
    println!("cargo:rerun-if-env-changed=BRUPOP_CONTAINER_IMAGE");
    println!("cargo:rerun-if-env-changed=BRUPOP_CONTAINER_IMAGE_PULL_SECRET");

    let path = PathBuf::from(YAMLGEN_DIR)
        .join("deploy")
        .join("bottlerocket-node-crd.yaml");
    let mut bottlerocket_node_crd = File::create(&path).unwrap();

    let path = PathBuf::from(YAMLGEN_DIR)
        .join("deploy")
        .join("brupop-apiserver.yaml");
    let mut brupop_apiserver = File::create(&path).unwrap();

    let path = PathBuf::from(YAMLGEN_DIR)
        .join("deploy")
        .join("brupop-agent.yaml");
    let mut brupop_agent = File::create(&path).unwrap();

    // testsys-crd related K8S manifest
    bottlerocket_node_crd.write_all(HEADER.as_bytes()).unwrap();
    serde_yaml::to_writer(&bottlerocket_node_crd, &BottlerocketNode::crd()).unwrap();

    let brupop_image = env::var("BRUPOP_CONTAINER_IMAGE").ok().unwrap();
    let brupop_image_pull_secrets = env::var("BRUPOP_CONTAINER_IMAGE_PULL_SECRET").ok();

    brupop_apiserver.write_all(HEADER.as_bytes()).unwrap();
    serde_yaml::to_writer(&brupop_apiserver, &brupop_namespace()).unwrap();
    serde_yaml::to_writer(&brupop_apiserver, &apiserver_service_account()).unwrap();
    serde_yaml::to_writer(&brupop_apiserver, &apiserver_cluster_role()).unwrap();
    serde_yaml::to_writer(&brupop_apiserver, &apiserver_cluster_role_binding()).unwrap();
    serde_yaml::to_writer(
        &brupop_apiserver,
        &apiserver_deployment(brupop_image.clone(), brupop_image_pull_secrets.clone()),
    )
    .unwrap();
    serde_yaml::to_writer(&brupop_apiserver, &apiserver_service()).unwrap();

    brupop_agent.write_all(HEADER.as_bytes()).unwrap();
    serde_yaml::to_writer(&brupop_agent, &brupop_namespace()).unwrap();
    serde_yaml::to_writer(&brupop_agent, &agent_service_account()).unwrap();
    serde_yaml::to_writer(&brupop_agent, &agent_cluster_role()).unwrap();
    serde_yaml::to_writer(&brupop_agent, &agent_cluster_role_binding()).unwrap();
    serde_yaml::to_writer(
        &brupop_agent,
        &agent_daemonset(brupop_image.clone(), brupop_image_pull_secrets.clone()),
    )
    .unwrap();
}
