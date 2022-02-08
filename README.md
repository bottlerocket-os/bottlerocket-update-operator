# Bottlerocket Update Operator

The Bottlerocket update operator is a [Kubernetes operator](https://Kubernetes.io/docs/concepts/extend-Kubernetes/operator/) that coordinates Bottlerocket updates on hosts in a cluster.
When installed, the Bottlerocket update operator starts a controller deployment on one node, an agent daemon set on every Bottlerocket node, and an Update Operator API Server deployment.
The controller orchestrates updates across your cluster, while the agent is responsible for periodically querying for Bottlerocket updates, draining the node, and performing the update when asked by the controller.
The agent performs all cluster object mutation operations via the API Server, which performs additional authorization using the Kubernetes TokenReview API -- ensuring that any request associated with a node is being made by the agent pod running on that node.
Updates to Bottlerocket are rolled out in [waves](https://github.com/bottlerocket-os/bottlerocket/tree/develop/sources/updater/waves) to reduce the impact of issues; the nodes in your cluster may not all see updates at the same time.

## Getting Started

### Installation

We can install the Bottlerocket update operator using the recommended configuration defined in [bottlerocket-update-operator.yaml](./bottlerocket-update-operator.yaml):

```sh
kubectl apply -f ./bottlerocket-update-operator.yaml
```

This will create the required namespace, custom resource definition, roles, deployments, etc., and use the latest update operator image available in [Amazon ECR Public](https://gallery.ecr.aws/bottlerocket/bottlerocket-update-operator).

### Label nodes

By default, each Workload resource constrains scheduling of the update operator by limiting pods to Bottlerocket nodes based on their labels.
These labels are not applied on nodes automatically and will need to be set on each using `kubectl`.
The agent relies on each node's updater components and schedules its pods based on their interface supported.
The node indicates its updater interface version in a label called `bottlerocket.aws/updater-interface-version`.
Agent deployments, respective to the interface version, are scheduled using this label and target only a single version in each.

For versions > `0.2.0` of the Bottlerocket update operator, only `update-interface-version` `2.0.0` is supported, which uses Bottlerocket's [update API](https://github.com/bottlerocket-os/bottlerocket/blob/develop/sources/updater/README.md#update-api) to dispatch updates.
For this reason, only Bottlerocket OS versions > `v0.4.1` are supported.

For the `2.0.0` `updater-interface-version`, this label looks like:

``` text
bottlerocket.aws/updater-interface-version=2.0.0
```

With [kubectl](https://kubernetes.io/docs/reference/kubectl/overview/) configured for the desired cluster, you can use the below command to get all nodes:

```sh
kubectl get nodes
```
Make a note of all the node names that you would like the Bottlerocket update operator to manage.

Next, add the `updater-interface-version` label to the nodes. 
For each node, use this command to add `updater-interface-version` label. 
Make sure to change `NODE_NAME` with the name collected from the previous command:

```sh
kubectl label node NODE_NAME bottlerocket.aws/updater-interface-version=2.0.0
```

If all nodes in the cluster are running Bottlerocket and require the same `updater-interface-version`, you can label all at the same time by running this:
```sh
kubectl label node $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}') bottlerocket.aws/updater-interface-version=2.0.0
```

If you must support `updater-interface-version` 1.0.0, please [open an issue](https://github.com/bottlerocket-os/bottlerocket-update-operator/issues/new/choose) and tell us about your use case.

### A Note About Removing Labels

Should you decide that the update operator should no longer manage a node, removing the `updater-interface-version` is not quite sufficient to remove the update operator components responsible for that node.
The update operator associates a Kubernetes Custom Resource with each node. While the Custom Resource will be garbage collected if the node itself is deleted, you must manually clean up the Custom Resource if you choose to delete only the `updater-interface-version`. The Custom Resource can be deleted like so:

```sh
# Set this to the name of the node you wish to stop managing with the update operator.
NODE_NAME="my-node-name"
kubectl delete brs brs-${NODE_NAME} --namespace brupop-bottlerocket-aws 
```

## Operation

### Overview

The update operator controller and agent processes communicate using Kubernetes [Custom Resources](https://kubernetes.io/docs/concepts/extend-kubernetes/api-extension/custom-resources/), with one being created for each node managed by the operator.
The Custom Resource created by the update operator is called a BottlerocketShadow resource, or otherwise shortened to `brs`.
The Custom Resource's Spec is configured by the controller to indicate a desired state, which guides the agent components.
The update operator's agent component keeps the Custom Resource Status updated with the current state of the node.
More about Spec and Status can be found in the [Kubernetes documentation](https://kubernetes.io/docs/concepts/overview/working-with-objects/kubernetes-objects/#object-spec-and-status).

Additionally, the update operator's controller and apiserver components expose metrics which can be configured to be [collected by Prometheus](#monitoring-cluster-history-and-metrics-with-prometheus).

### Observing State

#### Monitoring Custom Resources

The current state of the cluster from the perspective of the update operator can be summarized by querying the Kubernetes API for BottlerocketShadow objects.
This view will inform you of the current Bottlerocket version of each node managed by the update operator, as well as the current ongoing update status of any node with an update in-progress.
The following command requires `kubectl` to be configured for the development cluster to be monitored:

``` sh
kubectl get bottlerocketshadows --namespace brupop-bottlerocket-aws 
```

You can shorten this with:

``` sh
kubectl get brs --namespace brupop-bottlerocket-aws 
```

You should see output akin to the following:

```
$ kubectl get brs --namespace brupop-bottlerocket-aws 
NAME                                               STATE   VERSION   TARGET STATE   TARGET VERSION
brs-node-1                                         Idle    1.5.2     Idle           
brs-node-2                                         Idle    1.5.1     StagedUpdate   1.5.2
```

#### Monitoring Cluster History and Metrics with Prometheus

The update operator provides metrics endpoints which can be scraped into [Prometheus](https://prometheus.io/).
This allows you to monitor the history of update operations using popular metrics analysis and visualization tools.

We provide a [sample configuration](./yamlgen/telemetry/prometheus-resources.yaml) which demonstrates a Prometheus deployment into the cluster that is configured to gather metrics data from the update operator.

To deploy the sample configuration, you can use `kubectl`:

```sh
kubectl apply -f ./yamlgen/telemetry/prometheus-resources.yaml
```

Now that Prometheus is running in the cluster, you can use the UI provided to visualize the cluster's history.
Get the Prometheus pod name (e.g. `prometheus-deployment-5554fd6fb5-8rm25`):

```sh
kubectl get pods --namespace brupop-bottlerocket-aws 
```

Set up port forwarding to access Prometheus on the cluster:

```sh
kubectl port-forward $prometheus-pod-name 9090:9090 --namespace brupop-bottlerocket-aws 
```

Point your browser to `localhost:9090/graph` to access the sample Prometheus UI.

Search for:
* `brupop_hosts_state` to check how many hosts are in each state. 
* `brupop_hosts_version` to check how many hosts are in each Bottlerocket version.


### Image Region

`bottlerocket-update-operator.yaml` pulls operator images from Amazon ECR Public.
You may also choose to pull from regional Amazon ECR repositories such as the following.

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
  - 388230364387.dkr.ecr.us-gov-east-1.amazonaws.com
  - 347163068887.dkr.ecr.us-gov-west-1.amazonaws.com

### Current Limitations

- Communication between the bottlerocket agents and API server does not currently use SSL, due to a requirement for a cert management solution.
  We are considering an approach which uses [cert-manager](https://cert-manager.io) to that end.
- Monitoring on newly-rebooted nodes is limited.
  We are considering an approach in which custom health checks can be configured to run after reboots. (https://github.com/bottlerocket-os/bottlerocket/issues/503)
- single node cluster degrades into unscheduleable on update (https://github.com/bottlerocket-os/bottlerocket/issues/501)
- Node labels are not automatically applied to allow scheduling (https://github.com/bottlerocket-os/bottlerocket/issues/504)

## Troubleshooting

When installed with the [default deployment](./bottlerocket-update-operator.yaml), the logs can be fetched through Kubernetes deployment logs.
Because mutations to a node are orchestrated through the API server component, searching those deployment logs for a node ID can be useful.
To get logs for the API server, run the following:

```sh
kubectl logs deployment/brupop-apiserver --namespace brupop-bottlerocket-aws 
```

The controller logs will usually not help troubleshoot issues about the state of updates in a cluster, but they can similarly be fetched:

```sh
kubectl logs deployment/brupop-controller-deployment --namespace brupop-bottlerocket-aws 
```

### Why are updates stuck in my cluster?
The bottlerocket update operator only installs updates on one node at a time.
If a node's update becomes stuck, it can prevent the operator from proceeding with updates across the cluster.

The update operator uses the [Kubernetes Eviction API](https://kubernetes.io/docs/tasks/administer-cluster/safely-drain-node/) to safely drain pods from a node.
The eviction API will respect [PodDisruptionBudgets](https://kubernetes.io/docs/tasks/run-application/configure-pdb/), refusing to remove a pod from a node if it would cause a PDB not to be satisfied.
It is possible to mistakenly configure a Kubernetes cluster in such a way that a Pod can never be deleted while still maintaining the conditions of a PDB.
In this case, the operator may become stuck waiting for the PDB to allow an eviction to proceed.

Similarly, if the Node in question is repeatedly encountering issues while updating, it may cause updates across the cluster to become stuck.
Such issues can be troubleshooted by requesting the update operator agent logs from the node.
First, list the agent pods and select the pod residing on the node in question:

```sh
kubectl get pods --selector=brupop.bottlerocket.aws/component=agent -o wide --namespace brupop-bottlerocket-aws
```

Then fetch the logs for that agent:

```sh
kubectl logs brupop-agent-podname --namespace brupop-bottlerocket-aws 
```

### Why do only some of my Bottlerocket instances have an update available?

Updates to Bottlerocket are rolled out in [waves](https://github.com/bottlerocket-os/bottlerocket/tree/develop/sources/updater/waves) to reduce the impact of issues; the container instances in your cluster may not all see updates at the same time.
You can check whether an update is available on your instance by running the `apiclient update check` command from within the [control](https://github.com/bottlerocket-os/bottlerocket#control-container) or [admin](https://github.com/bottlerocket-os/bottlerocket#admin-container) container.

### Why do new container instances launch with older Bottlerocket versions?

The Bottlerocket update operator performs in-place updates for instances in your Kubernetes cluster.
The operator does not influence how those instances are launched.
If you use an [auto-scaling group](https://docs.aws.amazon.com/autoscaling/ec2/userguide/AutoScalingGroup.html) to launch your instances, you can update the AMI ID in your [launch configuration](https://docs.aws.amazon.com/autoscaling/ec2/userguide/LaunchConfiguration.html) or [launch template](https://docs.aws.amazon.com/autoscaling/ec2/userguide/LaunchTemplates.html) to use a newer version of Bottlerocket.

## How to Contribute and Develop Changes

Working on the update operator requires a fully configured and working Kubernetes cluster.
Because the agent component relies on the Bottlerocket API to properly function, we suggest a cluster which is running Bottlerocket nodes.
The `integ` crate can currently be used to launch Bottlerocket nodes into an [Amazon EKS](https://aws.amazon.com/eks/) cluster to observe update-operator behavior.

Have a look at the [design](./design/DESIGN.md) to learn more about how the update operator functions.
Please feel free to open an issue with an questions!

### Building and Deploying a Development Image

Targets in the `Makefile` can assist in creating an image.
The following command will build and tag an image using the local Docker daemon:

```sh
make brupop-image
```

Once this image is pushed to a container registry, you can set environment variables to regenerate a `.yaml` file suitable for deploying the image to Kubernetes.
Firstly, modify the `.env` file to contain the desired image name, as well as a secret for pulling the image if necessary.
Then run the following to regenerate the `.yaml` resource definitions:

```sh
cargo build -p yamlgen
```

These can of course be deployed using `kubectl apply` or the `cargo run --bin integ` for the integration testing tool.

## Security

See [CONTRIBUTING](CONTRIBUTING.md#security-issue-notifications) for more information.

## License

This project is dual licensed under either the Apache-2.0 License or the MIT license, your choice.
