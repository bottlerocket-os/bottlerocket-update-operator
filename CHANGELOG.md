# 1.1.0

## General

Added

* Removed OpenSSL in favor of Rust-based TLS using rustls ([#401])
* Updated TLS configurations to use leaf certs generated from root CA for brupop API server and agent ([#340])
* Added resource request limits for all containers ([#327])

Fixed

* Exposed the failure output for the `apiclient` when error occurs ([#342])
* `kube` clients are now created using the in-cluster DNS configuration ([#373])
* Removed deprecated Rust library APIs ([#403])
* Integration tests now use IMDSv2 calls ([#405])

Misc

* Numerous dependency upgrades and documentation fixes
* GitHub action workflows now use larger 16 core runners ([#356])

[#401]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/401
[#340]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/340
[#327]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/327
[#342]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/342
[#373]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/373
[#403]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/403
[#405]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/405
[#356]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/356

# 1.0.0

## General

Added

* Mechanism to constrain updates to a certain update time window ([#241])
* Option to exclude node before draining - ([#231])
* Port configuration ([#315])
* Support for concurrent updates - ([#238])
* Automatic prometheus scraping annotations for controller's service ([#269])
* Use `ca.crt` in SSL - ([#260])
* Reload certificates periodically to ensure no service loss ([#280])
* Replaced `bunyan` style logging in favor of human readable logs ([#298])
* Support webhook conversions from v2 to v1 (to support the Kubernetes pinwheel model) ([#308])
* Support integration tests in AWS China region ([#317]) ([#318])

Fixed

* Upgraded Bottlerocket SDK to consume fix for OpenSSl CVE-2022-3602 and CVE-2022-3786 ([#331])
* Gracefully exit Brupop agent when rebooting node ([#218])
* Clean up `bottlerocketshadows` when Brupop resources are removed from the cluster ([#235])
* Clarify crossbeam license ([#250])
* Made error handling module specific ([#279]) ([#291])

Misc

* Numerous dependency updates
* Fixed clippy linting / warnings ([#267])
* Clear and remove GitHub actions cache ([#268]) ([#286])
* Added step to integration tests to automatically add and delete cert-manager ([#320])
* Added GitHub action step to catch changes to deployment manifest ([#321])

[#218]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/218
[#231]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/231
[#235]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/235
[#238]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/238
[#241]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/241
[#250]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/250
[#260]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/260
[#267]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/267
[#268]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/268
[#269]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/269
[#279]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/279
[#280]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/280
[#286]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/286
[#291]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/291
[#298]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/298
[#308]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/308
[#315]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/315
[#317]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/317
[#318]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/318
[#320]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/320
[#321]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/321
[#331]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/331

# 0.2.2

## General

Added

* Add support to protect controller from becoming unscheduleable ([#14])
* Apply common k8s labels to all created resources ([#113])
* Support SSL communication between brupop-agent and brupop-apiserver ([#127])
* Handle update-reboot failures/ "crash loops" ([#161]), ([#123])
* Update README for setting up SSL ([#211])

Fixed

* Remove empty categories in Custom Resource spec ([#205])

## Integration test

Added

* Add README on integration test tool ([#166])
* Add integration testing subcommand Monitor which monitors new nodes for successful updates ([#130])
* Support integration test for IPv6 cluster ([#186])
* Improve integration testing subcommand Integration-test to creates the bottlerocket nodes via nodegroups ([#162])

Fixed

* Fixed integration test bugs ([#208]), ([#216])

[#14]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/14
[#113]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/113
[#123]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/123
[#127]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/127
[#130]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/130
[#161]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/161
[#162]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/162
[#166]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/166
[#186]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/186
[#205]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/205
[#208]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/208
[#211]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/211
[#216]: https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/216


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
