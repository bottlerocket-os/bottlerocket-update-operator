# Bottlerocket Update Operator Helm Chart

This helm chart carries the templates and resource definitions for
the bottlerocket-update-operator.

It depends on the `bottlerocket-shadow` CRD chart and should be installed
before the operator is installed.

In order for the bottlerocket update operator to register nodes as bottlerocket-shadow resources,
they need the `bottlerocket.aws/updater-interface-version` label:

```sh
kubectl label node NODE_NAME bottlerocket.aws/updater-interface-version=2.0.0
```

If all nodes in the cluster are running Bottlerocket and require the same `updater-interface-version`, you can label all at the same time by running this:
```sh
kubectl label node $(kubectl get nodes -o jsonpath='{.items[*].metadata.name}') bottlerocket.aws/updater-interface-version=2.0.0
```

Read more about labeling nodes,
[read the section in the project README.md on `Label nodes`.](https://github.com/bottlerocket-os/bottlerocket-update-operator#label-nodes)

### Local chart development

To install this chart locally, from the root of the `bottlerocket-update-operator` repository, run:

```
helm install \
  brupop \
  deploy/charts/bottlerocket-update-operator \
  --create-namespace
```

To install the bottlerocket-shadow CRD locally, run:
```
helm install \
  brupop-crd \
  deploy/charts/bottlerocket-shadow \
```

### Cert-manager

This helm chart depends on cert-manager being present on a cluster before install will succeed.
To get cert-manager using `kubectl`:

```sh
kubectl apply -f \
  https://github.com/cert-manager/cert-manager/releases/download/v1.8.2/cert-manager.yaml
```

Or to install via using helm:

```sh
# Add the cert-manager helm chart
helm repo add jetstack https://charts.jetstack.io

# Update your local chart cache with the latest
helm repo update

# Install the cert-manager (including it's CRDs)
helm install \
  cert-manager jetstack/cert-manager \
  --namespace cert-manager \
  --create-namespace \
  --version v1.8.2 \
  --set installCRDs=true
```

### Configuration

The following configuration values are supported:

```yaml
# Default values for bottlerocket-update-operator.

# The namespace to deploy the update operator into
namespace: "brupop-bottlerocket-aws"

# The image to use for brupop
image: "public.ecr.aws/bottlerocket/bottlerocket-update-operator:v1.2.0"

# If testing against a private image registry, you can set the pull secret to fetch images.
# This can likely remain as `brupop` so long as you run something like the following:
# kubectl create secret docker-registry brupop \
#  --docker-server 109276217309.dkr.ecr.us-west-2.amazonaws.com \
#  --docker-username=AWS \
#  --docker-password=$(aws --region us-west-2 ecr get-login-password) \
#  --namespace=brupop-bottlerocket-aws
#image_pull_secrets: |-
#  - name: "brupop"

# External load balancer setting.
# When `exclude_from_lb_wait_time_in_sec` is set to a positive value
# brupop will exclude the node from load balancing
# and will wait for `exclude_from_lb_wait_time_in_sec` seconds before draining nodes.
# Under the hood, this uses the `node.kubernetes.io/exclude-from-external-load-balancers` label
# to exclude those nodes from load balancing.
exclude_from_lb_wait_time_in_sec: "0"

# Concurrent update nodes setting.
# When `max_concurrent_updates` is set to a positive integer value,
# brupop will concurrently update max `max_concurrent_updates` nodes.
# When `max_concurrent_updates` is set to "unlimited",
# brupop will concurrently update all nodes with respecting `PodDisruptionBudgets`
# Note: the "unlimited" option does not work well with `exclude_from_lb_wait_time_in_sec`
# option, which could potential exclude all nodes from load balancer at the same time.
max_concurrent_updates: "1"

# DEPRECATED: use the scheduler settings
# Start and stop times for update window
# Brupop will operate node updates within update time window.
# when you set up time window start and stop time, you should use UTC (24-hour time notation).
update_window_start: "0:0:0"
update_window_stop: "0:0:0"

# Scheduler setting
# Brupop will operate node updates on scheduled maintenance windows by using cron expressions.
# When you set up the scheduler, you should follow cron expression rules.
# ┌───────────── seconds (0 - 59)
# │ ┌───────────── minute (0 - 59)
# │ │ ┌───────────── hour (0 - 23)
# │ │ │ ┌───────────── day of the month (1 - 31)
# │ │ │ │ ┌───────────── month (Jan, Feb, Mar, Apr, Jun, Jul, Aug, Sep, Oct, Nov, Dec)
# │ │ │ │ │ ┌───────────── day of the week (Mon, Tue, Wed, Thu, Fri, Sat, Sun)
# │ │ │ │ │ │ ┌───────────── year (formatted as YYYY)
# │ │ │ │ │ │ │
# │ │ │ │ │ │ │
# * * * * * * *
scheduler_cron_expression: "* * * * * * *"

# API server ports
# The port the API server uses for its own operations. This is accessed by the controller,
# the bottlerocket-shadow daemonset, etc.
apiserver_internal_port: "8443"
# API server internal address where the CRD version conversion webhook is served
apiserver_service_port: "443"

# Formatter for the logs emitted by brupop.
# Options are:
# * full - Human-readable, single-line logs
# * compact - A variant of full optimized for shorter line lengths
# * pretty - "Excessively pretty" logs optimized for human-readable terminal output.
# * json - Newline-delimited JSON-formatted logs.
logging_formatter: "pretty"
# Whether or not to enable ANSI colors on log messages.
# Makes the output "pretty" in terminals, but may add noise to web-based log utilities.
logging_ansi_enabled: "true"
# Controls the filter for tracing/log messages.
# This can be as simple as a log-level (e.g. "info", "debug", "error"), but also supports more complex directives.
# See https://docs.rs/tracing-subscriber/0.3.17/tracing_subscriber/filter/struct.EnvFilter.html#directives
controller_tracing_filter: "info"
agent_tracing_filter: "info"
apiserver_tracing_filter: "info"
```
