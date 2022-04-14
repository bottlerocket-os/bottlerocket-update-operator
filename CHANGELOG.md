# 0.2.1

Fixed:

* Fixed an issue where Node drains would hang indefinitely on StatefulSet Pods ([#168]), ([#179])
* Added more restrictive checking of TokenReviewStatus during apiserver auth

Added:

* Added support for IPv6 cluster ([#178])

[#168]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/168
[#179]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/179
[#178]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/178

# 0.2.0

Bottlerocket Update Operator (Brupop) 0.2.0 is a complete overhaul and rewrite of the update operator.
It will, by default, continue to rely on Bottlerocket’s client-side update API to determine when to perform an update on any given node — foregoing any complex deployment velocity controls, and instead relying on the wave system built-in to update Bottlerocket.
Compared to Brupop 0.1.0, Brupop 0.2.0 not only improves performance, but also increases observability while scoping down permissions required by the update operator agent.

When installed, the Bottlerocket update operator starts a controller deployment on one node, an agent daemon set on every Bottlerocket node, and an Update Operator API Server deployment.
The controller orchestrates updates across your cluster, while the agent is responsible for periodically querying for Bottlerocket updates, draining the node, and performing the update when asked by the controller.
Instead of having the independent controller and agent cooperate and pass messages via RPC, Brupop 0.2.0 associates a [Custom Resource](https://kubernetes.io/docs/concepts/extend-kubernetes/api-extension/custom-resources/) (called BottlerocketShadow) with each Bottlerocket node containing status information about the node, as well as a desired state.
The agent performs all cluster object mutation operations via the API Server.
[Service Account Token Volume Projection](https://kubernetes.io/docs/tasks/configure-pod-container/configure-service-account/#service-account-token-volume-projection) is used in API Server instead of the usual Kubernetes [rbac](https://kubernetes.io/docs/reference/access-authn-authz/rbac/) system for authorization to limit sufficient permissions for any node being able to modify any other nodes.

Brupop 0.2.0 also integrates with [Prometheus](https://prometheus.io/docs/instrumenting/clientlibs/) by exposing an HTTP endpoint from which Prometheus can gather metrics, allowing customers insight into the actions that the operator is taking. 


Fixed:

* Fixed a bug preventing nodes from being drained of certain pod deployments ([#74])
* Add more detailed context handling ([#71])
* Increased the amount of logging across the entirety of the operator ([#68])
* Added Prometheus metrics support ([#132])
* Added the ability to monitor cluster state by querying custom resources with kubectl ([#101]), ([#85])
* Simplified license scan and build process to use a single Dockerfile  ([#147])


Removed:

* Deprecated updog platform integration in favor of Bottlerocket API ([#60])

[#74]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/74
[#71]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/71 
[#68]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/68 
[#60]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/60
[#132]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/132
[#147]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/147
[#101]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/101
[#85]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/85 

# 0.1.5

* Use ECR Public image instead of region-specific image ([#65])
* Reduced memory and CPU limits for Agent pod. ([#55])
* Updated kubernetes client version ([#70])
* Updated Bottlerocket SDK version ([#63])

[#65]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/65
[#55]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/55
[#70]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/70
[#63]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/63

# 0.1.4

* Use bottlerocket update API to drive updates [#35] [#39]

To use the update API, nodes must be labeled with the `2.0.0` interface version:

```
bottlerocket.aws/updater-interface-version=2.0.0
```

To configure the use of the update API on all nodes in a cluster:

1. Ensure desired nodes are on bottlerocket `v0.4.1` or later

2. Set the `updater-interface-version` to `2.0.0` on nodes:

```bash
kubectl label node --overwrite=true $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}') bottlerocket.aws/updater-interface-version=2.0.0
```

* Add SELinux process label allowing API accesses by agent [#40]

* Fix deduplication filter in cases that could deadlock agent [#41]

[#35]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/35
[#39]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/39
[#40]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/40
[#41]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/41

# 0.1.3

* Add missing backtick in README instructions ([#25])
* Add license info to the operator container images ([#6])
* Specify passing `-c` to `watch` in README instructions for monitoring node status ([#27])
* Bump [bottlerocket-sdk](https://github.com/bottlerocket-os/bottlerocket-sdk) version to v0.10.1 for building the update operator's binaries. ([#21])
* Bump the version of the golang image to 1.14.1 to match the Go toolchain version in the [bottlerocket-sdk](https://github.com/bottlerocket-os/bottlerocket-sdk). ([#31])

This release includes a breaking change for users upgrading from v0.1.2:
* Change `platform-version` label to `updater-interface-version` for indicating updater interface version ([#30])

Please apply the new label on your bottlerocket nodes if you wish to use v0.1.3 of the update operator:
```
kubectl label node $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}') bottlerocket.aws/updater-interface-version=1.0.0
```

To remove the deprecated label from the nodes:
```
kubectl label nodes --all "bottlerocket.aws/platform-version"-
```

[#25]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/25
[#6]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/6
[#27]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/27
[#30]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/30
[#21]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/21
[#31]: https://github.com/bottlerocket-os/bottlerocket-update-operator/pull/31

# 0.1.2

Initial release of **bottlerocket-update-operator** - a Kubernetes operator that coordinates Bottlerocket updates on hosts in a cluster..

See the [README](README.md) for additional information.
