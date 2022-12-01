/*!

The custom resource definitions are modeled as Rust structs. Here we generate
the corresponding k8s yaml files.

!*/

use models::node::combined_crds;
use std::env;
use std::fs::File;
use std::path::PathBuf;

const DEPLOY_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn main() {
    // Re-run this build script if the model changes.
    println!("cargo:rerun-if-changed=../models/src");

    let path = PathBuf::from(DEPLOY_DIR)
        .join("tests")
        .join("golden")
        .join("custom-resource-definition.yaml");
    let brupop_shadow = File::create(path).unwrap();

    serde_yaml::to_writer(
        &brupop_shadow,
        &combined_crds("brupop-bottlerocket-aws".to_string(), "443".to_string()),
    )
    .unwrap();
}
