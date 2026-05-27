//! OTel metrics export for the request handler.

use crate::otel::{
    Gauge, GaugeDataPoint, Metric, MetricData, MetricNumber, ResourceMetrics, ScopeMetrics, Sum,
    SumDataPoint, Temporality, component_start, export_metrics_bindings, kv_str, now, resource,
    scope,
};

pub(crate) fn export_metrics(count: u64, response_len: usize, outbound_url: &str) {
    // Seed the cumulative start time BEFORE sampling `metric_time`, so on the
    // first export `start_time <= time` holds (OTel spec invariant for
    // cumulative sums).
    let start_time = component_start();
    let metric_time = now();
    let _ = export_metrics_bindings(&ResourceMetrics {
        resource: resource().clone(),
        scope_metrics: vec![ScopeMetrics {
            scope: scope().clone(),
            metrics: vec![
                Metric {
                    name: "http.server.request_count".into(),
                    description: "Total number of HTTP requests handled".into(),
                    unit: "{request}".into(),
                    data: MetricData::U64Sum(Sum {
                        data_points: vec![SumDataPoint {
                            attributes: vec![],
                            value: MetricNumber::U64(count),
                            exemplars: vec![],
                        }],
                        start_time,
                        time: metric_time,
                        temporality: Temporality::Cumulative,
                        is_monotonic: true,
                    }),
                },
                Metric {
                    name: "http.server.response_body.size".into(),
                    description: "Size of the fetched HTTP response body".into(),
                    unit: "By".into(),
                    data: MetricData::U64Gauge(Gauge {
                        data_points: vec![GaugeDataPoint {
                            attributes: vec![kv_str("http.target", outbound_url)],
                            value: MetricNumber::U64(response_len as u64),
                            exemplars: vec![],
                        }],
                        start_time: Some(start_time),
                        time: metric_time,
                    }),
                },
            ],
        }],
    });
}
