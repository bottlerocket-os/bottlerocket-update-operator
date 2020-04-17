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