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

You can install the chart via [the `wasmcloud-chart` package on ArtifactHub][artifacthub-wasmcloud]:

```console
$ helm install wasmcloud oci://ghcr.io/wasmcloud/wasmcloud-chart --version 0.7.2
```

> [!NOTE]
>
> You can replace `wasmcloud` with whatever you'd like to call your instantiation
> of the helm chart.
>
> Version `0.7.2` of the helm chart (released Jan 2nd, 2024) deploys version [`v0.81.0`][wasmcloud-v0.81.0] of the host by default.
>
> To change the version of the wasmcloud host that is deployed, set `wasmcloud.image.tag` in [`values.yaml`][values-yaml] to the
> [docker image tag][wasmcloud-docker-tags] you'd like to deploy instead.

This will get you up and going with a wasmCloud Host and NATS server so you can kick the tires and
try things out.

In order to access the dashboard and NATS, you'll need to forward the ports locally
(using the `RELEASE_NAME` you chose above as the `RELEASE_NAME` below)

```console
# In a second terminal
$ kubectl port-forward deployment/${RELEASE_NAME} 4222
```

[artifacthub-wasmcloud]: https://artifacthub.io/packages/helm/wasmcloud-chart/wasmcloud-chart
[wasmcloud-v0.81.0]: https://github.com/wasmCloud/wasmCloud/tree/v0.81.0
[wasmcloud-docker-tags]: https://hub.docker.com/r/wasmcloud/wasmcloud/tags
[values-yaml]: ./values.yaml

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

#### Kubernetes Applier Support

This chart comes with built in support for the [Kubernetes Applier provider and
component](https://github.com/cosmonic/kubernetes-applier). To enable support so that any applier
provider running on these nodes automatically gets the necessary credentials, set
`wasmcloud.enableApplierSupport` to `true`. Note that this will force usage of a pod
`ServiceAccount`.

If using the architecture described in the [applier
documentation](https://github.com/cosmonic/kubernetes-applier/tree/main/service-applier#requirements-for-hosts-running-in-kubernetes)
that uses router nodes, you can use the `wasmcloud.customLabels` map to set the custom labels needed
for those hosts (such as `wasmcloud.dev/route-to: "true"`)
