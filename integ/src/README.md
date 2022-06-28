# Brupop Integration Test

## Introduction
Integration test is a tool that helps build automated brupop integration testing, which consists of three main subcommands *Integration-test*, *Monitor*, *and Clean*.

### Integration-test subcommand

This allows you to set up Brupop test environment, complete Brupop installation, and label nodes.

```
cargo run --bin integ integration-test --cluster-name <YOUR_CLUSTER_NAME> --region <YOUR_CLUSTER_REGION> --bottlerocket-version <OLD_BOTTLEROCKET_VERSION>  --arch <ARCH> --nodegroup-name <NODEGROUP_NAME>
```

### Monitor subcommand
This allows you to verify if that nodes are being updated to target Bottlerocket version

```
cargo run --bin integ monitor --cluster-name <YOUR_CLUSTER> --region <YOUR_CLUSTER_REGION>
```

### Clean subcommand
This allows you to destroy all resources which were created when integration test installed brupop.

```
cargo run --bin integ clean --cluster-name <YOUR_CLUSTER_NAME> --region <YOUR_CLUSTER_REGION> --nodegroup-name <NODEGROUP_NAME>
```