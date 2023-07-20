# Bottlerocket Shadow

Bottlerocket shadows are the "reflections" of the bottlerocket nodes themselves.

They are used by the Bottlerocket update operator to perform update operations through the
host's `apiclient update` interface.

### Local chart development

To install the CRD locally:
```
helm install \
  brupop-crd \
  deploy/charts/bottlerocket-shadow \
```

[_Don't forget to label your Bottlerocket nodes_](https://github.com/bottlerocket-os/bottlerocket-update-operator#label-nodes)
so the Brupop controller can start operating on them!

You can use the following to label _all_ nodes in your cluster
with the `bottlerocket.aws/updater-interface-version=2.0.0` label

```
kubectl label node $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}') bottlerocket.aws/updater-interface-version=2.0.0
```

### Configuration

The following configuration values are supported:

```yaml
# The namespace to deploy the update operator into
namespace: "brupop-bottlerocket-aws"

# API server internal address where the conversion webhook is served
apiserver_service_port: "443"
```
