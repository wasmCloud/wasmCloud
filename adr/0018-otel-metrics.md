# Add OTEL-compliant metrics to wasmCloud

| Status   | Deciders                                                                | Date        |
|----------|-------------------------------------------------------------------------|-------------|
| Accepted | Commenters on [#664](https://github.com/wasmCloud/wasmCloud/issues/664) | 17 Feb 2024 |

## Context

After much development and refinement, [OpenTelemetry][otel] has become *the* standard for [Observability][o11y] in the Cloud Native ecosystem, and the three primary "pillars of observability" are key:

- Logs
- Tracing
- Metrics

While wasmCloud has supported logs and tracing for a long time with the integration of the [`tracing`][crates-tracing], metrics has not been suported natively by the wasmCloud host, and thus not supported by wasmCloud as a whole.

Logs and tracing are sufficient to solve *most* problems that actually occur with a running host, but metrics are important to ease the burden of operators of any given service, so implementing them is important. Tools like [Prometheus][prom] and [Grafana][grafana] have become industry standards, and are used by DevOps practitioners and operations people at large extensively across the industry.

[otel]: https://opentelemetry.io/
[o11y]: https://en.wikipedia.org/wiki/Observability_(software)
[crates-tracing]: https://crates.io/crates/tracing
[prom]: https://prometheus.io/docs
[grafana]: https://grafana.com/

## Problem Statement

wasmCloud should support OTEL-compliant metrics, along with logs and tracing.

## Decision Drivers

* Ease of monitoring for operators
* Insight into the key metrics to monitor for wasmCloud hosts
* Support and integrate with the OTEL-compliant observability metrics collection ecosystem

## Considered Options

* Support OTEL-compliant metrics
* Not implementing OTEL based metrics support
* Supporting a different stnadard for metrics

## Decision Outcome

Chosen option: "Support OTEL-compliant metrics", because integrating properly with the rest of the cloud native ecosystem around metrics and making operations easier encourages usage of wasmCloud and improves the product.

### Positive Consequences

* Easier maintenance of wasmCloud hosts
* Build a base on which to add more important metrics
* Interoperation with other cloud native ecosystem projects

### Negative Consequences <!-- optional -->

* More complexity (annotations, etc) in the `wasmCloud/wasmCloud` codebase
* Likely more requests for metrics to be exposed and some related friction

## Links <!-- optional -->

* [Original RFC](https://github.com/wasmCloud/wasmCloud/issues/664)
* [Initial implementation PR with the bulk of the implementation (contributed by @joonas)](https://github.com/wasmCloud/wasmCloud/pull/1431)
