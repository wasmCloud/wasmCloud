# OTEL-HTTP

A WebAssembly component that demonstrates how to use [OpenTelemetry](https://opentelemetry.io/) within a Wasm component via the `wasi:otel` interfaces. The component implements an HTTP counter service and instruments each operation with OTel tracing, logging, and metrics.

## Prerequisites

- `wash` CLI tool must be installed

## OpenTelemetry Usage

This component exercises all three pillars of observability through `wasi:otel`:

### Tracing

- Retrieves the host's span context with `outer-span-context` for trace propagation
- Creates a parent `handle-request` server span covering the full request lifecycle
- Creates child client spans for each downstream operation (HTTP fetch, blobstore write, keyvalue increment)
- Attaches semantic attributes and span events to each span

### Logging

- Emits structured `LogRecord`s via `on-emit` with severity level, JSON body, and resource/scope metadata
- Correlates every log record with the active trace using `trace-id` and `span-id`

### Metrics

- Exports a monotonic `U64Sum` counter tracking the total number of requests handled
- Exports a `U64Gauge` capturing the latest HTTP response body size

## Features

The underlying HTTP counter service:

1. Reads runtime configuration keys using `wasi:config`
2. Makes an outgoing HTTP request to `https://example.com` using `wasi:http/outgoing-handler`
3. Stores the response body in a blob container using `wasi:blobstore`
4. Increments a request counter using `wasi:keyvalue`
5. Returns the current count as a plain text HTTP response (or HTTP 500 on failure)

## WASI Interfaces

**Imports:**
- `wasi:otel/tracing` - Span lifecycle (on-start, on-end, outer-span-context)
- `wasi:otel/logs` - Structured log emission (on-emit)
- `wasi:otel/metrics` - Metrics export (export)
- `wasi:otel/types` - Shared telemetry types (KeyValue, Resource, InstrumentationScope)
- `wasi:clocks/wall-clock` - Timestamps for spans, logs, and metrics
- `wasi:http/outgoing-handler` - Outbound HTTP requests
- `wasi:blobstore` - Persistent blob storage
- `wasi:keyvalue` - Key-value store and atomic counters
- `wasi:config` - Runtime configuration access
- `wasi:logging` - Basic leveled logging

**Exports:**
- `wasi:http/incoming-handler` - Handles incoming HTTP requests

## Building

```bash
wash build
```

## Running

```bash
wash dev
```
