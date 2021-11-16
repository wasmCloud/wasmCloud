# wasmCloud Host Helm Chart

A Helm Chart for installing and scaling wasmCloud hosts inside of Kubernetes.

## Overview

In keeping with our goal of "compatible with, but not dependent on," this Helm chart provides an
option to users who would like to run wasmCloud hosts inside of pods on Kubernetes. Please note this
is not the required or recommended way for completely new development, but is meant to be used as a
way to bridge those already running things in Kubernetes with the new work being done in wasmCloud!
If you are trying something completely net new, running your hosts in Kubernetes might not be your
best option.

### Limitations

This chart is purely meant for creating a pool of scalable hosts and not for running your whole
lattice. Ideally, the main NATS server and the place you'll be controlling your lattice from will
likely be outside of Kubernetes. If you'd like to run everything inside of Kubernetes, we'd
recommend deploying your main NATS server with another chart.

## Running the chart

Right now this chart is not available within a chart repository. We will find a place for it soon
and update installation instructions. To use this chart, you'll need to clone the repository and
then run from the root of the repo:

```console
$ helm install <RELEASE_NAME> wasmcloud_host/chart
```

This will get you up and going with a wasmCloud Host and NATS server so you can kick the tires and
try things out. In order to access the dashboard and NATS, you'll need to forward the ports locally
(using the `RELEASE_NAME` you chose above as the `RELEASE_NAME` below)

```console
# In one terminal
$ kubectl port-forward deployment/${RELEASE_NAME} 4000

# In a second terminal
$ kubectl port-forward deployment/${RELEASE_NAME} 4222
```

### Configuration

For a full list of configuration options, see the documented `values.yaml` file.

#### Production usage

By default, the chart ships with a standalone NATS server and a `Service` that could be exposed
through a `LoadBalancer` or `Ingress`. However, this is not meant for production usage. Currently, 2
main deployment methods are supported: leaf node and external NATS server

##### Leaf Node

This option allows you to run a small NATS [leaf
node](https://docs.nats.io/nats-server/configuration/leafnodes) alongside the wasmCloud Host. This
is a common deployment practice when connecting with something like [NGS](https://synadia.com/ngs).
To use this method, you'll need to set `nats.leafnode.enabled` to `true`. Additionally, you'll need
to set the following values on the command line or in a values file:

| Value                       | Purpose                                                                                                                                                                 |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `nats.leafnode.clusterURL`  | The URL of the NATS cluster to connect to                                                                                                                               |
| `nats.leafnode.credentials` | The credentials for connecting to the NATS cluster. If passing on the command line, we recommend using `--set-file`                                                     |
| `nats.jetstreamDomain`      | The JetStream domain to use for distributing cache data. If you are using domains, you will also need to set a different domain for `.wasmcloud.config.jetstreamDomain` |

##### External NATS Server

If you'd like to connect directly to a NATS server instead, you can disable the NATS sidecare
container by setting `nats.enabled` to `false`. You can then set the various config options
available under `wasmcloud.config` to use your NATS server with the credentials you have generated
for the host. Please see the `values.yaml` documentation as well as the [wasmCloud Host
documentation](https://wasmcloud.dev/reference/host-runtime/host_configure/) for more detailed
information.

##### Scaling the hosts

If you have deployed the host using one of the production options, it can be scaled as high as you'd
like. The number of hosts can be scaled by setting `replicaCount` to the desired number or by using
`kubectl scale`
