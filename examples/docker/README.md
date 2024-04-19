# Examples - Docker

This directory contains configuration for [Docker][docker] and [Docker Compose][docker-compose] which are helpful for running wasmCloud and related examples. 

The images used include:

- The wasmCloud host itself (ex. ``)
- A [NATS](https://nats.io) server with JetStream enabled which supports the network for a lattice (ex. ``)
- A [wasmCloud Application Deployment Manager (WADM)](/docs/category/declarative-application-deployment-wadm) server (ex. ``)
- An OCI registry ([`distribution`][distribution]), to support pushing and pulling locally-built artifacts (ex. ``)
- [Grafana][grafana] for visualizing telemetry data (ex. ``)
- [Tempo](https://grafana.com/oss/tempo/) for collecting distributed traces (ex. ``)
- [Prometheus][prometheus] for collecting metrics (ex. ``)

[docker]: https://docs.docker.com/engine
[docker-compose]: https://docs.docker.com/compose
[distribution]: https://github.com/docker/distribution
[tempo]: https://grafana.com/oss/tempo
[grafana]: https://grafana.com/oss/grafana
[prometheus]: https://prometheus.com/oss/prometheus

## Running the minimal wasmCloud infrastructure

The minimum required infrastructure for wasmCloud is a NATS server and a wasmCloud host.

```bash
docker compose -f docker-compose.minimal.yml up
```

## Running the entire ecosystem in Docker

```bash
docker compose -f docker-compose-full.yml up
```

## Running the entire ecosystem in Docker (with a websocket port open)
> A WebSocket connection is needed for the "Washboard UI" to work

```bash
docker compose -f docker-compose-websockets.yml up
```

## Running supporting services in Docker
The auxiliary services include a local registry for managing OCI components and Grafana and Tempo in order to view distributed traces.

```bash
docker compose -f docker-compose-auxiliary.yml up
OTEL_TRACES_EXPORTER=otlp wash up # start the host with OTEL exports enabled
```

## Viewing Telemetry data

### Viewing traces collected by Tempo

You can view traces collected by [Tempo][tempo] in the [Grafana][grafana] dashboard.

Navigate to http://localhost:5050/explore, click the dropdown in the upper left, and select "Tempo".

There are several ways to query traces in Tempo. To see all traces from the host, change the "Query type" tab to "Search", then click the "Service Name" dropdown and select "wasmcloud-host." You can also increase the "Limit" field to something more than the default (20).

To search, press Shift-Enter. You can click on any of the Trace IDs to view all the spans associated with the trace.

### Viewing metrics collected by Prometheus

There are two different ways to query metrics collected by [Prometheus][prometheus] (after being emitted from the wasmCloud host):

1. Prometheus' built-in query interface at http://localhost:9090/graph
2. Grafana's "Explore" interface at http://localhost:5050/explore (Once you are in the "View", select "Prometheus" from the dropdown on the top-left corner and under the "Metric" field select the metric you would like to explore)

If you are in the process of emitting new metrics, or you are interested in exploring the metrics in a Prometheus-native interface, the Prometheus built-in query interface is probably the better fit.

If you are interested in seeing how the metrics would look in an interface that you might be looking at them in production, or you are looking to develop a metrics dashboard, the Grafana interface is the better fit.
