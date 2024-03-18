# Bottlerocket Update Operator

The Bottlerocket update operator (or, for short, Brupop) is a [Kubernetes operator](https://Kubernetes.io/docs/concepts/extend-Kubernetes/operator/) that coordinates Bottlerocket updates on hosts in a cluster.
When installed, the [Bottlerocket update operator starts a controller deployment on one node, an agent daemon set on every Bottlerocket node, and an Update Operator API Server deployment](https://bottlerocket.dev/en/brupop/latest#/concepts/#controlled-updates).
The controller orchestrates updates across your cluster, while the agent is responsible for periodically querying for Bottlerocket updates, draining the node, and performing the update when asked by the controller.
The agent performs all cluster object mutation operations via the API Server, which performs additional authorization using the Kubernetes TokenReview API -- ensuring that any request associated with a node is being made by the agent pod running on that node.
Further, `cert-manager` is required in order for the API server to use a CA certificate to communicate over SSL with the agents.
Updates to Bottlerocket are rolled out in [waves](https://github.com/bottlerocket-os/bottlerocket/tree/develop/sources/updater/waves) to reduce the impact of issues; the nodes in your cluster may not all see updates at the same time.

## User Documentation

You can find Brupop’s user documentation on [bottlerocket.dev](https://bottlerocket.dev/en/brupop/).


This new, expanded documentation covers how to [setup](https://bottlerocket.dev/en/brupop/latest#/setup/), [install](https://bottlerocket.dev/en/brupop/latest#/setup/install/), [configure](https://bottlerocket.dev/en/brupop/latest#/setup/configure/) and [use](https://bottlerocket.dev/en/brupop/latest#/operate/) Brupop on your Bottlerocket clusters.
The new documentation is versioned and cross-linked to the other Bottlerocket documentation, so you can quickly find information on your versions of the cluster and operator.

For convenience and linking, you'll find a [mapping between content previously described in this document and new documentation](#previous-documentation-links).

### Monitoring Cluster History and Metrics with Prometheus

The update operator provides metrics endpoints which can be scraped by [Prometheus](https://prometheus.io/).
This allows you to monitor the history of update operations using popular metrics analysis and visualization tools.

We provide a [sample configuration](./deploy/examples/prometheus-resources.yaml) which demonstrates a Prometheus deployment into the cluster that is configured to gather metrics data from the update operator.

To deploy the sample configuration, you can use `kubectl`:

```sh
kubectl apply -f ./deploy/telemetry/prometheus-resources.yaml
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



## Developer documentation

For a deep dive on installing Brupop, how it works, and its integration with Bottlerocket, [check out this design deep dive document!](./design/1.0.0-release.md)

#### Configuration via Kubernetes yaml

While most users will want to configure Brupop via helm as there are fewer opportunities for misconfiguration, the the Kubernetes YAML configuration provides insight into the components of Brupop.

##### Configure API server ports

If you'd like to configure what ports the API server uses,
adjust the value that is consumed in the container environment:

```yaml
      ...
      containers:
        - command:
            - "./api-server"
          env:
            - name: APISERVER_INTERNAL_PORT
              value: 999
```

You'll then also need to adjust the various "port" entries in the YAML manifest
to correctly reflect what port the API server starts on and expects for its service port:

```yaml
    ...
    webhook:
      clientConfig:
        service:
          name: brupop-apiserver
          namespace: brupop-bottlerocket-aws
          path: /crdconvert
          port: 123
```

The default values are generated from the [default values yaml file](https://github.com/bottlerocket-os/bottlerocket-update-operator/blob/develop/deploy/bottlerocket-update-operator/values.yaml) file:
- `apiserver_internal_port: "8443"` - This is the container port the API server starts on.
  If this environment variable is _not_ found, the Brupop API server will fail to start.
- `apiserver_service_port: "443"` - This is the port Brupop's Kubernetes Service uses to target the internal API Server port.
  It is used by the node agents to access the API server.
  If this environment variable is _not_ found, the Brupop agents will fail to start.

##### Resource Requests & Limits

The `bottlerocket-update-operator.yaml` manifest makes several default recommendations for
Kubernetes resource requests and limits. In general, the update operator and its components are lite-weight
and shouldn't consume more than 10m CPU (which is roughly equivalent to 1/100th of a CPU core)
and 50Mi (which is roughly equivalent to 0.05 GB of memory).
If this limit is breached, the Kubernetes API will restart the faulting container.

Note that your mileage with these resource requests and limits may vary.
Any number of factors may contribute to varying results in resource utilization (different compute instance types, workload utilization, API ingress/egress, etc).
[The Kubernetes documentation for Resource Management of Pods and Containers](https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/)
is an excellent resource for understanding how various compute resources are utilized
and how Kubernetes manages these resources.

If resource utilization by the brupop components is not a concern,
removing the `resources` fields in the manifest will not affect the functionality of any components.

##### Exclude Nodes from Load Balancers Before Draining
This configuration uses Kubernetes [ServiceNodeExclusion](https://kubernetes.io/docs/reference/command-line-tools-reference/feature-gates/) feature.
`EXCLUDE_FROM_LB_WAIT_TIME_IN_SEC` can be used to enable the feature to exclude the node from load balancer before draining.
When `EXCLUDE_FROM_LB_WAIT_TIME_IN_SEC` is 0 (default), the feature is disabled.
When `EXCLUDE_FROM_LB_WAIT_TIME_IN_SEC` is set to a positive integer, bottlerocket update operator will exclude the node from
load balancer and then wait `EXCLUDE_FROM_LB_WAIT_TIME_IN_SEC` seconds before draining the pods on the node.

To enable this feature, set the `exclude_from_lb_wait_time_in_sec` value in your helm values yaml file to a positive integer. For example,
`exclude_from_lb_wait_time_in_sec: "100"`.

Otherwise, go to `bottlerocket-update-operator.yaml` and change `EXCLUDE_FROM_LB_WAIT_TIME_IN_SEC` to a positive integer value.
For example:
```yaml
      ...
      containers:
        - command:
            - "./agent"
          env:
            - name: MY_NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
            - name: EXCLUDE_FROM_LB_WAIT_TIME_IN_SEC
              value: "180"
      ...
```

##### Set Up Max Concurrent Update

`MAX_CONCURRENT_UPDATE` can be used to specify the max concurrent updates during updating.
When `MAX_CONCURRENT_UPDATE` is a positive integer, the bottlerocket update operator
will concurrently update up to `MAX_CONCURRENT_UPDATE` nodes respecting [PodDisruptionBudgets](https://kubernetes.io/docs/tasks/run-application/configure-pdb/).
When `MAX_CONCURRENT_UPDATE` is set to `unlimited`, bottlerocket update operator
will concurrently update all nodes respecting [PodDisruptionBudgets](https://kubernetes.io/docs/tasks/run-application/configure-pdb/).

Note: The `MAX_CONCURRENT_UPDATE` configuration does not work well with `EXCLUDE_FROM_LB_WAIT_TIME_IN_SEC` 
configuration, especially when `MAX_CONCURRENT_UPDATE` is set to `unlimited`, it could potentially exclude all 
nodes from load balancer at the same time.

To enable this feature, set the `max_concurrent_updates` value in your helm values yaml file to a positive integer value or `unlimited`. For example,
`max_concurrent_updates: "1"` or `max_concurrent_updates: "unlimited"`.

Otherwise, go to `bottlerocket-update-operator.yaml` and change `MAX_CONCURRENT_UPDATE` to a positive integer value or `unlimited`.
For example:
```yaml
      containers:
        - command:
            - "./controller"
          env:
            - name: MY_NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
            - name: MAX_CONCURRENT_UPDATE
              value: "1"
```

##### Set scheduler
`SCHEDULER_CRON_EXPRESSION` can be used to specify the scheduler in which updates are permitted.
When `SCHEDULER_CRON_EXPRESSION` is "* * * * * * *" (default), the feature is disabled.

To enable this feature, set the `scheduler_cron_expression` value in your helm values yaml file.
This value should be a valid cron expression.
A cron expression can be configured to a time window or a specific trigger time.
When users specify cron expressions as a time window, the bottlerocket update operator will operate node updates within that update time window.
When users specify cron expression as a specific trigger time, brupop will update and complete all waitingForUpdate nodes on the cluster when that time occurs.
```
# ┌───────────── seconds (0 - 59)
# | ┌───────────── minute (0 - 59)
# | │ ┌───────────── hour (0 - 23)
# | │ │ ┌───────────── day of the month (1 - 31)
# | │ │ │ ┌───────────── month (Jan, Feb, Mar, Apr, Jun, Jul, Aug, Sep, Oct, Nov, Dec)
# | │ │ │ │ ┌───────────── day of the week (Mon, Tue, Wed, Thu, Fri, Sat, Sun)
# | │ │ │ │ │ ┌───────────── year (formatted as YYYY)
# | │ │ │ │ │ |
# | │ │ │ │ │ |
# * * * * * * *
```

Note: brupop uses Coordinated Universal Time(UTC), please convert your local time to Coordinated Universal Time (UTC). This tool [Time Zone Converter](https://www.timeanddate.com/worldclock/converter.html) can help you find your desired time window on UTC.
For example (schedule to run update operator at 03:00 PM on Monday ):
```yaml
      containers:
        - command:
            - "./controller"
          env:
            - name: MY_NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
            - name: SCHEDULER_CRON_EXPRESSION
              value: "* * * * * * *"
```

##### Set an Update Time Window - DEPRECATED

**Note**: these settings are deprecated and will be removed in a future release.
Time window settings cannot be used in combination with the preferred cron expression format and will be ignored.

If you still decide to use these settings, please use "hour:00:00" format only instead of "HH:MM:SS".

`UPDATE_WINDOW_START` and `UPDATE_WINDOW_STOP` can be used to specify the time window in which updates are permitted.

To enable this feature, set the `update_window_start` and `update_window_stop` values in your helm values yaml file to a `hour:minute:second` formatted value (UTCE 24-hour time notation).
For example: `update_window_start: "08:0:0"` and `update_window_stop: "12:30:0"`.

Otherwise, go to `bottlerocket-update-operator.yaml` and change `UPDATE_WINDOW_START` and `UPDATE_WINDOW_STOP` to a `hour:minute:second` formatted value (UTC (24-hour time notation)). 

Note that `UPDATE_WINDOW_START` is inclusive and `UPDATE_WINDOW_STOP` is exclusive.

Note: brupop uses UTC (24-hour time notation), please convert your local time to UTC.
For example:
```yaml
      containers:
        - command:
            - "./controller"
          env:
            - name: MY_NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
            - name: UPDATE_WINDOW_START
              value: "09:00:00"
            - name: UPDATE_WINDOW_STOP
              value: "21:00:00"
```


### How to Contribute and Develop Changes

Working on the update operator requires a fully configured and working Kubernetes cluster.
Because the agent component relies on the Bottlerocket API to properly function, we suggest a cluster which is running Bottlerocket nodes.
The `integ` crate can currently be used to launch Bottlerocket nodes into an [Amazon EKS](https://aws.amazon.com/eks/) cluster to observe update-operator behavior.

Have a look at the [design](./design/DESIGN.md) to learn more about how the update operator functions.
Please feel free to open an issue with an questions!

### Building and Deploying a Development Image

To re-generate the yaml manifest found at the root of this reposiroy, simply run:
```
make manifest
```
Note: this requires `helm` to be installed.

Targets in the `Makefile` can assist in creating an image.
The following command will build and tag an image using the local Docker daemon:

```sh
make brupop-image
```

Once this image is pushed to a container registry, you can set environment variables to regenerate a `.yaml` file suitable for deploying the image to Kubernetes.
Firstly, modify the `.env` file to contain the desired image name, as well as a secret for pulling the image if necessary.
Then run the following to regenerate the `.yaml` resource definitions:

```sh
cargo build -p deploy
```

These can of course be deployed using `kubectl apply` or the automatic integration testing tool [integ](https://github.com/bottlerocket-os/bottlerocket-update-operator/tree/develop/integ).

### Current Limitations

- Monitoring on newly-rebooted nodes is limited.
  We are considering an approach in which custom health checks can be configured to run after reboots. (https://github.com/bottlerocket-os/bottlerocket/issues/503)
- Node labels are not automatically applied to allow scheduling (https://github.com/bottlerocket-os/bottlerocket/issues/504)

### Image Region

`bottlerocket-update-operator.yaml` pulls operator images from Amazon ECR Public.
You may also choose to pull from regional Amazon ECR repositories such as the following.

  - `917644944286.dkr.ecr.af-south-1.amazonaws.com`
  - `375569722642.dkr.ecr.ap-east-1.amazonaws.com`
  - `328549459982.dkr.ecr.ap-northeast-1.amazonaws.com`
  - `328549459982.dkr.ecr.ap-northeast-2.amazonaws.com`
  - `328549459982.dkr.ecr.ap-northeast-3.amazonaws.com`
  - `328549459982.dkr.ecr.ap-south-1.amazonaws.com`
  - `328549459982.dkr.ecr.ap-southeast-1.amazonaws.com`
  - `328549459982.dkr.ecr.ap-southeast-2.amazonaws.com`
  - `386774335080.dkr.ecr.ap-southeast-3.amazonaws.com`
  - `328549459982.dkr.ecr.ca-central-1.amazonaws.com`
  - `328549459982.dkr.ecr.eu-central-1.amazonaws.com`
  - `328549459982.dkr.ecr.eu-north-1.amazonaws.com`
  - `586180183710.dkr.ecr.eu-south-1.amazonaws.com`
  - `328549459982.dkr.ecr.eu-west-1.amazonaws.com`
  - `328549459982.dkr.ecr.eu-west-2.amazonaws.com`
  - `328549459982.dkr.ecr.eu-west-3.amazonaws.com`
  - `553577323255.dkr.ecr.me-central-1.amazonaws.com`
  - `509306038620.dkr.ecr.me-south-1.amazonaws.com`
  - `328549459982.dkr.ecr.sa-east-1.amazonaws.com`
  - `328549459982.dkr.ecr.us-east-1.amazonaws.com`
  - `328549459982.dkr.ecr.us-east-2.amazonaws.com`
  - `328549459982.dkr.ecr.us-west-1.amazonaws.com`
  - `328549459982.dkr.ecr.us-west-2.amazonaws.com`
  - `388230364387.dkr.ecr.us-gov-east-1.amazonaws.com`
  - `347163068887.dkr.ecr.us-gov-west-1.amazonaws.com`
  - `183470599484.dkr.ecr.cn-north-1.amazonaws.com.cn`
  - `183901325759.dkr.ecr.cn-northwest-1.amazonaws.com.cn`

Example regional image URI:
```
328549459982.dkr.ecr.us-west-2.amazonaws.com/bottlerocket-update-operator:v1.1.0
```

## Version Support Policy

As per our policy, Brupop follows the semantic versioning (semver) principles, ensuring that any updates in minor versions do not introduce any breaking or backward-incompatible changes. However, please note that we only provide security patches for the latest minor version. Therefore, it is highly recommended to always keep your Brupop installation up to date with the latest available version.

For example: If `v1.3.0` is the latest Brupop release, then, `v1.3` (latest minor version) will be considered as supported and `v1.3.0` (latest available version) will be the recommended version of Brupop to be installed. When `v1.3.1` is released, then that version will be considered as recommended.

## Previous documentation links

To retain pre-existing links and bookmarks to this README, the following headings are presented here to guide you to the most up-to-date documentation.
Update your links and bookmarks accordingly.

#### Getting Started

The previous "Getting Started" section is now covered in the [setup](https://bottlerocket.dev/en/brupop/latest#/setup/) documentation.

#### Installation

The previous "Installation" section is now covered in [prerequisite](https://bottlerocket.dev/en/brupop/latest#/setup/cert-manager/) and [install](https://bottlerocket.dev/en/brupop/latest#/setup/install/) documentation.

#### Configuration

The previous "Configuration" section is now covered in the [configuration](https://bottlerocket.dev/en/brupop/latest#/setup/configure) documentation.

##### Configure via Helm values yaml file

The previous "Configure via Helm values yaml file" section is now covered in the [optional configuration](https://bottlerocket.dev/en/brupop/latest#/setup/configure/#optional-configuration) documentation.
YAML manifest configuration can be found on this README as part of the developer-oriented information under the heading [Configuration via Kubernetes yaml](#configuration-via-kubernetes-yaml).

#### Label nodes

The previous "Label nodes" section is now covered under the ["Label Nodes" heading in the configuration](https://bottlerocket.dev/en/brupop/latest#/setup/configure/#label-nodes) documentation.

##### Automatic labeling via Bottlerocket user-data

The previous "Automatic labeling via Bottlerocket user-data" section is now covered under the ["Labeling a node with the Bottlerocket API" heading](https://bottlerocket.dev/en/brupop/latest#/setup/configure/#labeling-a-node-with-the-bottlerocket-api) in the [configuration documentation](https://bottlerocket.dev/en/brupop/latest#/setup/configure/).

##### Automatic labeling via `eksctl`

The previous "Automatic labeling via `eksctl`" section is now covered under the ["Label all nodes when starting an EKS cluster with `eksctl`" heading](
https://bottlerocket.dev/en/brupop/latest#/setup/configure/#label-all-nodes-when-starting-an-eks-cluster-with-eksctl) in the [configuration documentation](https://bottlerocket.dev/en/brupop/latest#/setup/configure/).

#### Uninstalling

The previous "Uninstalling" section is now covered in the [Disable/Uninstall](https://bottlerocket.dev/en/brupop/latest#/uninstall/) documentation.

#### Operation

The previous "Operation" section is now covered in the [Operate](https://bottlerocket.dev/en/brupop/latest#/operate/) documentation.

##### Overview

The previous "Overview" section is now covered in the [Operate](https://bottlerocket.dev/en/brupop/latest#/operate/) documentation.

##### Observing State

The previous "Observing State" section is now covered in the [Operate](https://bottlerocket.dev/en/brupop/latest#/operate/) documentation.

##### Monitoring Custom Resources

The previous "Monitoring Custom Resources" section is now covered under the heading ["Adhoc Query"](https://bottlerocket.dev/en/brupop/latest#/operate/#adhoc-query) in the [Operate](https://bottlerocket.dev/en/brupop/latest#/operate/) documentation.

#### Troubleshooting

The previous "Troubleshooting" section is now covered in the [Troubleshoot](https://bottlerocket.dev/en/brupop/latest#/troubleshoot/) documentation.

##### Why are updates stuck in my cluster?

The previous heading "Why are updates stuck in my cluster?" is now covered under the heading “Stuck Updates” in the [Troubleshoot](https://bottlerocket.dev/en/brupop/latest#/troubleshoot/) documentation.

##### Why are my bottlerocket nodes egressing to `https://updates.bottlerocket.aws`?

The previous heading "Why are my bottlerocket nodes egressing to `https://updates.bottlerocket.aws`?" is now covered in the [Bottlerocket FAQ](https://bottlerocket.dev/en/faq/#7_2).

##### Why do only some of my Bottlerocket instances have an update available?

The previous heading “Why do only some of my Bottlerocket instances have an update available?” is now covered in the [Bottlerocket FAQ](https://bottlerocket.dev/en/faq/#7_3).

##### Why do new container instances launch with older Bottlerocket versions?

The previous heading “Why do new container instances launch with older Bottlerocket versions?” is now covered under the heading “Bottlerocket instances start with an old version of Bottlerocket”  in the [Troubleshoot](https://bottlerocket.dev/en/brupop/latest#/troubleshoot/) documentation.

## Security

See [CONTRIBUTING](CONTRIBUTING.md#security-issue-notifications) for more information.

## License

This project is dual licensed under either the Apache-2.0 License or the MIT license, your choice.
