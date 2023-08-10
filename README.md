# Bottlerocket Update Operator Helm Repository

**[https://bottlerocket-os.github.io/bottlerocket-update-operator](https://bottlerocket-os.github.io/bottlerocket-update-operator)**


## What is the Bottlerocket Update Operator?

The Bottlerocket update operator (or, for short, Brupop) is a [Kubernetes operator](https://Kubernetes.io/docs/concepts/extend-Kubernetes/operator/) that coordinates Bottlerocket updates on hosts in a cluster.
When installed, the Bottlerocket update operator starts a controller deployment on one node, an agent daemon set on every Bottlerocket node, and an Update Operator API Server deployment.
The controller orchestrates updates across your cluster, while the agent is responsible for periodically querying for Bottlerocket updates, draining the node, and performing the update when asked by the controller.
The agent performs all cluster object mutation operations via the API Server, which performs additional authorization using the Kubernetes TokenReview API -- ensuring that any request associated with a node is being made by the agent pod running on that node.

## Installation

See the [project's README](https://github.com/bottlerocket-os/bottlerocket-update-operator/blob/develop/README.md) for installation instructions

## License

This project is dual licensed under either the Apache-2.0 License or the MIT license, your choice.
