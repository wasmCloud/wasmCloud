# wasmCloud Benchmarking Chart

This chart is primarily used for benchmarking wasmCloud deployments within a Kubernetes cluster. It
can be used to test something outside of the cluster as well, but this is not a tested use case.
This chart has a self contained stack of Grafana, Loki, Tempo, OTEL collectors, and Prometheus as
well as installation for the [K6 Operator](https://github.com/grafana/k6-operator) for running the
actual benchmark. This setup is meant to be temporary as many users are likely to have their own
Grafana or OTEL stacks. This setup is purely for capturing data for the test. By default, this is a
"push button" chart that will install the stack and run the benchmark for you. It can then be
uninstalled so as to not take up resources.

## Prerequisites

Before running this chart, you should have a working wasmCloud install running in your cluster.
Please follow the [docs](https://wasmcloud.com/docs/deployment/k8s/) for information on how to do
this. Once you have wasmCloud running and you have a component deployed and accessible via a
Kubernetes `Service` resource, you can proceed with installing this chart.

## Basic Usage

A simple install command looks like below:

```bash
helm upgrade --install my-benchmark --version 0.1.0 oci://ghcr.io/wasmcloud/charts/benchmark --wait --set test.url=http://hello-world:8000
```

Please note that the `test.url` value is required. This is the URL that the benchmark will use to
test against. This should be a URL that is accessible from within the Kubernetes cluster. Generally
speaking, this should be a component you have deployed and exposed with either the built-in HTTP
provider or the HTTP server provider. The `--wait` flag should also be used so the k6 CRD and
controller can start up before running the benchmark.

The output from the helm install has instructions for how to view results and the dashboard. For the
install command above, the output would look like this:

```
The k6 benchmark should now be running! To get the logs and output of the test, you can run:

kubectl logs -n default -l k6_cr=my-benchmark-test,runner=true --tail=-1

If you'd like to view dashboards during or after your tests, port-forward to the Grafana instance:

kubectl port-forward -n default svc/my-benchmark-grafana 3000:80

Then open http://localhost:3000 in your browser and navigate to the "Test Environment" dashboard in the dashboards section.
```

## Advanced Usage

There are also several advanced options available for customizing the chart and test runs. A full
example of modifying several options is available in the [example
values.yaml](./example-values.yaml) file.

### Customizing the test

When running large load tests, it is recommended that you run the k6 tests on a separate node pool
from where your wasmCloud hosts are running so as to avoid biasing the results. You can do this by
using the node selector and toleration options as shown below:

```yaml
tolerations:
  - effect: NoSchedule
    key: pool
    operator: Equal
    value: benchmark
nodeSelector:
  pool: benchmark
```

You can also configure various options for the k6 test. All of these options are described in the
table below:

| Parameter          | Description                                                 | Default Value |
| ------------------ | ----------------------------------------------------------- | ------------- |
| `test.enabled`     | Controls automatic generation and execution of k6 test.     | `true`        |
| `test.url`         | Target URL for k6 GET requests - Required to be set by user | `null`        |
| `test.parallelism` | Number of parallel k6 test runners to deploy                | `3`           |
| `test.separate`    | Whether to run each test runner on a separate node          | `false`       |

You can also add or override test scenarios that will be generated and run by the chart. This can be
done using the map `test.scenarios`. This map should follow the same format as the [k6 test
scenarios](https://grafana.com/docs/k6/latest/using-k6/scenarios/) in either JSON or YAML format.
The default test scenario is at the key `default`. An example of a custom test scenario is shown
below:

```yaml
test:
  scenarios:
    default:
      rate: 4000
    10k:
      executor: "constant-arrival-rate"
      timeUnit: "1s"
      duration: "1m"
      startTime: "1m"
      preAllocatedVUs: 50
      maxVUs: 1000
      rate: 8000
```

### Running your own tests

If you would like to run your own tests, you can disable the automatic generation and execution of
the k6 test by setting `test.enabled` to `false`. This will set up all of the infrastructure for you
to run the tests without actually running anything. You can then follow the [k6
documentation](https://grafana.com/docs/k6/latest/set-up/set-up-distributed-k6/usage/) to create
your own `TestRun` objects and run them manually. Please note that this is an advanced use case and
you'll need to wire up the test output to the configured OTEL collector. Below is an example of the
generated `TestRun` object for the command in the [basic usage](#basic-usage) section:

```yaml
apiVersion: k6.io/v1alpha1
kind: TestRun
metadata:
  name: my-benchmark-test
spec:
  arguments: -o experimental-opentelemetry
  parallelism: 3
  runner:
    env:
    - name: K6_OTEL_GRPC_EXPORTER_INSECURE
      value: "true"
    - name: K6_OTEL_GRPC_EXPORTER_ENDPOINT
      value: my-benchmark-opentelemetry-collector:4317
    - name: K6_OTEL_METRIC_PREFIX
      value: k6_
    - name: K6_NO_USAGE_REPORT
      value: "true"
    - name: GOMEMLIMIT
      valueFrom:
        resourceFieldRef:
          resource: limits.memory
    - name: GOMAXPROCS
      valueFrom:
        resourceFieldRef:
          resource: limits.cpu
  script:
    configMap:
      file: test.js
      name: my-benchmark-test-config
  separate: false
```

If creating your own test, use this as reference for how to connect to the configured OTEL
collector.

## Local Development

If running/testing the chart locally, the steps are mostly the same as the [basic
usage](#basic-usage) section. However, you need to fetch the dependencies for the chart first:

```bash
helm dependency update
helm upgrade --install my-benchmark . --wait --set test.url=http://hello-world:8000
```
