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
