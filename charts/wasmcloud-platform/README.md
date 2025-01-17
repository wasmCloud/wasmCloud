# wasmcloud-platform

![Version: 0.1.0](https://img.shields.io/badge/Version-0.1.0-informational?style=flat-square) ![Type: application](https://img.shields.io/badge/Type-application-informational?style=flat-square) ![AppVersion: 1.2.1](https://img.shields.io/badge/AppVersion-1.2.1-informational?style=flat-square)

[wasmCloud](https://wasmcloud.com/docs/intro) is an open source project from the Cloud Native Computing Foundation (CNCF) that enables teams to build polyglot applications composed of reusable Wasm components and run them—resiliently and efficiently—across any cloud, Kubernetes, datacenter, or edge.

The wasmCloud Platform Helm Chart provides a turnkey solution, for running WebAssembly applications on Kubernetes.

## Prerequisites

*   Kubernetes: `>=1.24.0`

*   Helm: `>=3.8.0`

*   If installing the NATS server - a StatefulSet - the means of provisioning PVCs (e.g. a CSI driver)

## Getting Started

The wasmCloud platform is comprised of three components, a NATS cluster as the backbone of its [lattice](https://wasmcloud.com/docs/concepts/lattice), [Wadm](https://wasmcloud.com/docs/ecosystem/wadm) for WebAssembly applications lifecycle management, and wasmCloud [host](https://wasmcloud.com/docs/concepts/hosts), which can be provided either by [wasmcloud-operator](https://github.com/wasmcloud/wasmcloud-operator) - if preferring managed hosts - or [wasmcloud-host](https://github.com/wasmCloud/wasmCloud/tree/main/charts/wasmcloud-host).

To take full advantage of the chart features, it's best to use wasmcloud-operator, and install the chart in two steps.

### Deploy wasmCloud Platform

```bash
# By default, the chart installs NATS, Wadm, and wasmCloud Operator subcharts
helm upgrade --install \
    wasmcloud-platform \
    --values https://raw.githubusercontent.com/wasmCloud/wasmcloud/main/charts/wasmcloud-platform/values.yaml \
    oci://ghcr.io/wasmcloud/charts/wasmcloud-platform \
    --dependency-update
```

Wait for all components to install and wadm-nats communications to establish:

```bash
kubectl rollout status deploy,sts -l app.kubernetes.io/name=nats
kubectl wait --for=condition=available --timeout=600s deploy -l app.kubernetes.io/name=wadm
kubectl wait --for=condition=available --timeout=600s deploy -l app.kubernetes.io/name=wasmcloud-operator
```

### Create a wasmCloud Host

```bash
helm upgrade --install \
    wasmcloud-platform \
    --values https://raw.githubusercontent.com/wasmCloud/wasmcloud/main/charts/wasmcloud-platform/values.yaml \
    oci://ghcr.io/wasmcloud/charts/wasmcloud-platform \
    --dependency-update \
    --set "hostConfig.enabled=true"
```

Check the status of the newly created host:

```bash
kubectl describe wasmcloudhostconfig wasmcloud-host
```

To validate the wasmCloud platform's readiness for WebAssembly applications, you can follow the wasmCloud documentation to [run](https://wasmcloud.com/docs/deployment/k8s/kind#run-a-webassembly-component-on-kubernetes) a sample WebAssembly application on the host, and [test](https://wasmcloud.com/docs/deployment/k8s/kind#test-the-application) it.

## Values

| Key                                                 | Type   | Default                                                  | Description                                                                                                                                                                        |
| --------------------------------------------------- | ------ | -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| nats.enabled                                        | bool   | `true`                                                   | Whether to install the nats subchart                                                                                                                                               |
| ***nats chart configuration values***               | chart  |                                                          | For the most up to date information, please see the official NATS.io helm chart on ArtifactHUB: <https://artifacthub.io/packages/helm/nats/nats>                                   |
| wadm.enabled                                        | bool   | `true`                                                   | Whether to install the wadm subchart                                                                                                                                               |
| ***wadm chart configuration values***               | chart  |                                                          | For the most up to date information, please see the Wadm helm chart on GitHub: Source: <https://github.com/wasmCloud/wasmcloud-operator/main/examples/quickstart/wadm-values.yaml> |
| operator.enabled                                    | bool   | `true`                                                   | Whether to install the wasmcloud-operator subchart                                                                                                                                 |
| ***wasmcloud-operator chart configuration values*** | chart  |                                                          | For the most up to date information, please see the wasmcloud-operator helm chart on GitHub: <https://github.com/wasmCloud/wasmcloud-operator/main/charts/wasmcloud-operator>      |
| host.enabled                                        | bool   | `false`                                                  | Whether to install the wasmcloud-host subchart                                                                                                                                     |
| ***wasmcloud-host chart configuration values***     | chart  |                                                          | For the most up to date information, please see the wasmcloud-host helm chart on GitHub: <https://github.com/wasmCloud/wasmCloud/main/charts/wasmcloud-host>                       |
| hostConfig.enabled                                  | bool   | `false`                                                  | Whether to use the wasmCloud host configuration custom resource. This requires the WasmCloudHostConfig CRD, which is part of wasmcloud-operator.                                   |
| hostConfig.name                                     | string | `"wasmcloud-host"`                                       | Name of the wasmCloud host configuration resource.                                                                                                                                 |
| hostConfig.namespace                                | string | `"default"`                                              | Namespace to deploy the wasmCloud host to.                                                                                                                                         |
| hostConfig.hostReplicas                             | int    | `1`                                                      | Number of hosts (pods).                                                                                                                                                            |
| hostConfig.lattice                                  | string | `"default"`                                              | The lattice to connect the hosts to.                                                                                                                                               |
| hostConfig.hostLabels                               | object | `{}`                                                     | Additional labels to apply to the host other than the defaults set in the controller.                                                                                              |
| hostConfig.version                                  | string | `"latest"`                                               | Which wasmCloud version to use.                                                                                                                                                    |
| hostConfig.image                                    | string | `""`                                                     | If not provided, the image corresponding to the `version` will be used.                                                                                                            |
| hostConfig.natsLeafImage                            | string | `""`                                                     | If not provided, the default upstream image will be used.                                                                                                                          |
| hostConfig.secretName                               | string | `""`                                                     | The name of a secret containing a set of NATS credentials under 'nats.creds' key.                                                                                                  |
| hostConfig.natsCredentialsFile                      | string | `""`                                                     | The file containing the NATS access credentials; if provided, the file must be placed within the chart's main directory or one of its subdirectories.                              |
| hostConfig.enableStructuredLogging                  | bool   | `false`                                                  | Enable structured logging for host logs.                                                                                                                                           |
| hostConfig.registryCredentialsSecret                | string | `""`                                                     | The name of a secret containing the registry credentials.                                                                                                                          |
| hostConfig.registryCredentialsFile                  | string | `""`                                                     | The file containing the login credentials for the private registry where wasmCloud host images are stored.                                                                         |
| hostConfig.controlTopicPrefix                       | string | `"wasmbus.ctl"`                                          | The control topic prefix to use for the host.                                                                                                                                      |
| hostConfig.leafNodeDomain                           | string | `"leaf"`                                                 | The leaf node domain to use for the NATS sidecar.                                                                                                                                  |
| hostConfig.configServiceEnabled                     | bool   | `false`                                                  | Makes wasmCloud host issue requests to a config service on startup.                                                                                                                |
| hostConfig.logLevel                                 | string | `"INFO"`                                                 | The log level to use for the host.                                                                                                                                                 |
| hostConfig.natsAddress                              | string | `"nats://nats-headless.default.svc.cluster.local"`                | The address of the NATS server to connect to.                                                                                                                                      |
| hostConfig.allowLatest                              | bool   | `false`                                                  | Allow the host to deploy using the latest tag on OCI components or providers.                                                                                                      |
| hostConfig.allowedInsecure                          | list   | `[]`                                                     | Allow the host to pull artifacts from OCI registries insecurely.                                                                                                                   |
| hostConfig.policyService.topic                      | string | `""`                                                     | If provided, enables policy checks on start actions and component invocations.                                                                                                     |
| hostConfig.policyService.changesTopic               | string | `""`                                                     | If provided, allows the host to subscribe to updates on past policy decisions. Requires 'topic' to be set.                                                                         |
| hostConfig.policyService.timeoutMs                  | int    | `1000`                                                   | If provided, allows setting a custom timeout for requesting policy decisions; defaults to 1000. Requires 'topic' to be set.                                                        |
| hostConfig.observability.enable                     | bool   | `true`                                                   | Enables all signals (logs/metrics/traces) at once. Set it to 'false' and enable each signal individually in case you don't need all of them.                                       |
| hostConfig.observability.endpoint                   | string | `"otel-collector.svc"`                                   | The OpenTelemetry collector endpoint configuration.                                                                                                                                |
| hostConfig.observability.protocol                   | string | `"http"`                                                 | The OpenTelemetry collector protocol (http, grpc).                                                                                                                                 |
| hostConfig.observability.logs.enable                | bool   | `false`                                                  | Use if setting observability signals individually.                                                                                                                                 |
| hostConfig.observability.logs.endpoint              | string | `"logs-specific-otel-collector.svc"`                     |                                                                                                                                                                                    |
| hostConfig.observability.metrics.enable             | object | `false`                                                  | Use if setting observability signals individually.                                                                                                                                 |
| hostConfig.observability.metrics.endpoint           | string | `"metrics-specific-otel-collector.svc"`                  |                                                                                                                                                                                    |
| hostConfig.observability.traces.enable              | object | `false`                                                  | Use if setting observability signals individually.                                                                                                                                 |
| hostConfig.observability.traces.endpoint            | string | `"traces-specific-otel-collector.svc"`                   |                                                                                                                                                                                    |
| hostConfig.secretsTopicPrefix                       | string | `"wasmcloud.secrets"`                                    | For more context, please see:  <https://wasmcloud.com/docs/concepts/secrets>                                                                                                       |
| hostConfig.maxLinearMemoryBytes                     | int    | `20000000`                                               | The maximum amount of memory bytes that a component can allocate.                                                                                                                  |
| hostConfig.schedulingOptions.daemonset              | bool   | `false`                                                  | Whether to run the wasmCloud host as a DaemonSet                                                                                                                                   |
| hostConfig.schedulingOptions.resources              | object | `{"nats":{},"wasmCloudHost":{}}`                         | See <https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/> for valid values                                                                              |
| hostConfig.schedulingOptions.podTemplateAdditions   | object | `{"spec":{"nodeSelector":{"kubernetes.io/os":"linux"}}}` | Note that you *cannot* set the `containers` field here as it is managed by the controller.                                                                                         |

