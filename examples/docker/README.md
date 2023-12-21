## Overview

This directory contains Docker Compose files for starting containers helpful for running wasmCloud examples. These containers include:

- a [NATS](https://nats.io) server with JetStream enabled, needed to support the network for a lattice
- an OCI registry, to support pushing and pulling locally-built artifacts
- [Grafana](https://grafana.com/) + [Tempo](https://grafana.com/oss/tempo/), for viewing distributed traces
- the wasmCloud host
- a [WADM](/docs/category/declarative-application-deployment-wadm) server, for managing wasmCloud applications

## Running the entire ecosystem in Docker

```bash
docker compose -f docker-compose-full.yml up
```

## Running supporting services in Docker

```bash
docker compose -f docker-compose-auxiliary.yml up
OTEL_TRACES_EXPORTER=otlp wash up # start the host with OTEL exports enabled
```

## Viewing traces

Navigate to http://localhost:5050/explore, click the dropdown in the upper left, and select "Tempo".

There are several ways to query traces in Tempo. To see all traces from the host, change the "Query type" tab to "Search", then click the "Service Name" dropdown and select "wasmCloud Host." You can also increase the "Limit" field to something more than the default (20).

To search, press Shift-Enter. You can click on any of the Trace IDs to view all the spans associated with the trace.
