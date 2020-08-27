# Bottlerocket Update Operator

The Bottlerocket update operator is a [Kubernetes operator](https://Kubernetes.io/docs/concepts/extend-Kubernetes/operator/) that coordinates Bottlerocket updates on hosts in a cluster.

## How to Run on Kubernetes

To run the update operator in a Kubernetes cluster, the following are required resources and configuration ([suggested deployment is defined in `update-operator.yaml`](./update-operator.yaml)):

- **Update operator's container image**

  Holding the Operator's binaries and supporting environment (CA certificates).

- **Controller deployment**

  Schedules a stop-restart-tolerant controller process on available nodes.

- **Agent daemon set**

  Schedules agent on Bottlerocket hosts

- **Bottlerocket namespace**

  Groups Bottlerocket related resources and roles.

- **Service account for the agent**

  Used for authenticating the agent process on Kubernetes APIs.

- **Cluster privileged credentials with read-write access to nodes for the agent**

  Grants the agent's service account permissions to update annotations for its node.

- **Service account for the controller**

  Used for authenticating the controller process on Kubernetes APIs.

- **Cluster privileged credentials with access to pods and nodes for controller**

  Grants the controller's service account permissions to update annotations and manage pods that are scheduled on nodes (to cordon & drain) before and after updating.

Cluster administrators can deploy the update operator with [suggested configuration defined here](./update-operator.yaml) - this includes the above resources and Bottlerocket published container images.
With `kubectl` configured for the desired cluster, the suggested deployment can made with:
  
``` sh
kubectl apply -f ./update-operator.yaml
```

Once the deployment's resources are in place, there is one more step needed to schedule and place the required pods on Bottlerocket nodes.
By default - in the suggested deployment, each Workload resource constrains scheduling of the update operator by limiting pods to Bottlerocket nodes based on their labels.
These labels are not applied on nodes automatically and will need to be set on each using `kubectl`.
The agent relies on each node's updater components and schedules its pods based on their interface supported.
The node indicates its updater interface version in a label called `bottlerocket.aws/updater-interface-version`.
Agent deployments, respective to the interface version, are scheduled using this label and target only a single version in each.

For the `1.0.0` `updater-interface-version`, this label looks like:

``` text
bottlerocket.aws/updater-interface-version=1.0.0
```

`kubectl` can be used to set this label on a node in the cluster:

``` sh
kubectl label node $NODE_NAME bottlerocket.aws/updater-interface-version=1.0.0
```

If all nodes in the cluster are running Bottlerocket and have the same `updater-interface-version`, all can be labeled at the same time:

``` sh
kubectl label node $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}') bottlerocket.aws/updater-interface-version=1.0.0
```

Each workload resource may have additional constraints or scheduling affinities based on each node's labels in addition to the `bottlerocket.aws/updater-interface-version` label scheduling constraint.

Customized deployments may use the [suggested deployment](./update-operator.yaml) or the [example development deployment](./dev/deployment.yaml) as a starting point, with customized container images specified if needed.

## Scheduled Components

The update operator system is deployed as set of a replica set (for the controller) and a daemon set (for the agent). 
Each runs their respective process configured as either a `-controller` or an `-agent`:

- `bottlerocket-update-operator -controller`

  The coordinating process responsible for the handling update of Bottlerocket nodes
  cooperatively with the cluster's workloads.

- `bottlerocket-update-operator -agent`

  The on-host process responsible for publishing update metadata and executing
  update activities.

## Coordination

The update operator controller and agent processes communicate by updating the node's annotations as the node steps through an update.
The node's annotations are used to communicate an `intent` which acts as a goal or target that is set by the controller.
The controller uses internal policy checks to manage which `intent` should be communicated to an agent.
This allows the controller to fully own and coordinate each step taken by agents throughout its cluster.
No agent process will otherwise take any disruptive or intrusive action without being directed by the controller to do so (in fact the agent is limited to periodic metadata updates *only*).

To handle and respond to `intent`s, the agent and controller processes subscribe to Kubernetes' node resource update events.
These events are emitted whenever update is made on the subscribed to resource, including: heartbeats, other node status changes (pods, container image listing), and metadata changes (labels and annotations).


### Observing State

The update operator's state can be closely monitored through the labels and annotations on node resources.
The state and pending activity are updated as progress is being made.
The following command requires `kubectl` to be configured for the development cluster to be monitored and `jq` to be available on `$PATH`.

``` sh
kubectl get nodes -o json \
  | jq -C -S '.items | map(.metadata|{(.name): (.annotations*.labels|to_entries|map(select(.key|startswith("bottlerocket.aws")))|from_entries)}) | add'
```

There is a `get-nodes-status` `Makefile` target provided for monitoring nodes during development.
Note: the same dependencies and assumptions for the above command apply here.

```sh
# get the current status:
make get-nodes-status

# or periodically (handy for watching closely):
watch -c -- make get-nodes-status
```

### Image Region

`update-operator.yaml` pulls operator images from the `us-west-2` region.
To pull from a different region, replace the image endpoint to point at one of the following endpoints.
This is not required, but pulling the image from the same region as your cluster will improve the download speed.

  - 917644944286.dkr.ecr.af-south-1.amazonaws.com
  - 375569722642.dkr.ecr.ap-east-1.amazonaws.com
  - 328549459982.dkr.ecr.ap-northeast-1.amazonaws.com
  - 328549459982.dkr.ecr.ap-northeast-2.amazonaws.com
  - 328549459982.dkr.ecr.ap-south-1.amazonaws.com
  - 328549459982.dkr.ecr.ap-southeast-1.amazonaws.com
  - 328549459982.dkr.ecr.ap-southeast-2.amazonaws.com
  - 328549459982.dkr.ecr.ca-central-1.amazonaws.com
  - 328549459982.dkr.ecr.eu-central-1.amazonaws.com
  - 328549459982.dkr.ecr.eu-north-1.amazonaws.com
  - 586180183710.dkr.ecr.eu-south-1.amazonaws.com
  - 328549459982.dkr.ecr.eu-west-1.amazonaws.com
  - 328549459982.dkr.ecr.eu-west-2.amazonaws.com
  - 328549459982.dkr.ecr.eu-west-3.amazonaws.com
  - 509306038620.dkr.ecr.me-south-1.amazonaws.com
  - 328549459982.dkr.ecr.sa-east-1.amazonaws.com
  - 328549459982.dkr.ecr.us-east-1.amazonaws.com
  - 328549459982.dkr.ecr.us-east-2.amazonaws.com
  - 328549459982.dkr.ecr.us-west-1.amazonaws.com
  - 328549459982.dkr.ecr.us-west-2.amazonaws.com

### Current Limitations

- pod replication & healthy count is not taken into consideration (https://github.com/bottlerocket-os/bottlerocket/issues/502)
- nodes update without pause between each node (https://github.com/bottlerocket-os/bottlerocket/issues/503)
- single node cluster degrades into unscheduleable on update (https://github.com/bottlerocket-os/bottlerocket/issues/501)
- node labels are not automatically applied to allow scheduling (https://github.com/bottlerocket-os/bottlerocket/issues/504)

## How to Contribute and Develop Changes

Working on the update operator requires a fully configured & working Kubernetes cluster.
For the sake of development workflow, we suggest using a cluster that is containerized or virtualized.
There are helpful tools available to manage these: [`kind`](https://github.com/Kubernetes-sigs/kind) for containerized clusters and [`minikube`](https://github.com/Kubernetes/minikube) for locally virtualized clusters.
The `dev/` directory contains several resources that may be used for development and debugging purposes:

- `dashboard.yaml` - **development** dashboard deployment (**using insecure settings, not a suitable production deployment**)
- `deployment.yaml` - _template_ for Kubernetes resources that schedule a controller's `ReplicaSet` and agent's `DaemonSet`
- `kind-cluster.yml` - `kind` cluster definition that may be used to stand up a local development cluster

Much of the development workflow can be driven by the `Makefile` in the root of the repository.
Each of the `Makefile`'s' targets use tools and environments that they're configured to access - for example: `kubectl`, as configured on a host, will be used.
If `kubectl` is configured to configured with access to production, please take steps to configure `kubectl` to target a development cluster.

**Build targets**

- `build` - build executable using go toolchain in `$PATH`
- `test` - run `go test` for the operator using go toolchain in `$PATH`
- `container` - build a container image for use in Kubernetes resources
- `container-test` - run update operator's unit tests in a container
- `check` - run checks for container image
- `dist` - create a distribution archive of the container image
- `clean` - remove cached build artifacts from workspace

**Development targets**

- `dashboard` - create or update Kubernetes-dashboard (**not suitable for use in production**)
- `deploy-dev` - create or update the operator's Kubernetes resources
- `rollout` - reload and restart the operator's pods

**`kind` development targets**

- `kind-cluster` - create a local [`kind`](https://github.com/Kubernetes-sigs/kind) cluster
- `kind-load` - build and load container image for use in a `kind` cluster
- `kind-rollout` - reload container image & config, then restart pods
