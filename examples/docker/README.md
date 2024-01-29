## Overview

This directory contains Docker Compose files for starting containers helpful for running wasmCloud examples. These containers include:

- a [NATS](https://nats.io) server with JetStream enabled, needed to support the network for a lattice
- the wasmCloud host
- a [WADM](/docs/category/declarative-application-deployment-wadm) server, for managing wasmCloud applications
- an OCI registry, to support pushing and pulling locally-built artifacts
- [Grafana](https://grafana.com/) + [Tempo](https://grafana.com/oss/tempo/), for viewing distributed traces

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

## Viewing traces

Navigate to http://localhost:5050/explore, click the dropdown in the upper left, and select "Tempo".

There are several ways to query traces in Tempo. To see all traces from the host, change the "Query type" tab to "Search", then click the "Service Name" dropdown and select "wasmcloud-host." You can also increase the "Limit" field to something more than the default (20).

To search, press Shift-Enter. You can click on any of the Trace IDs to view all the spans associated with the trace.

## Viewing metrics

We provide two different ways to query metrics that are being emitted by the wasmCloud Host, you can either access them via:

1. Prometheus' built-in query interface at http://localhost:9090/graph.
2. Grafana's "Explore" interface at http://localhost:5050/explore. Once you are in the "View", select "Prometheus" from the dropdown on the top-left corner and under the "Metric" field select the metric you would like to explore.

Depending on what you are trying to accomplish with the metrics, one or the other interface may be a better fit for you.

If you are in the process of emitting new metrics, or you are interested in exploring the metrics in a Prometheus-native interface, the Prometheus built-in query interface is probably the better fit.

If you are interested in seeing how the metrics would look in an interface that you might be looking at them in production, or you are looking to develop a metrics dashboard, the Grafana interface is the better fit.