package provider

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"go.opentelemetry.io/otel/exporters/otlp/otlplog/otlploggrpc"
	"go.opentelemetry.io/otel/exporters/otlp/otlplog/otlploghttp"
	"go.opentelemetry.io/otel/exporters/otlp/otlpmetric/otlpmetricgrpc"
	"go.opentelemetry.io/otel/exporters/otlp/otlpmetric/otlpmetrichttp"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracegrpc"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/sdk/log"
	"go.opentelemetry.io/otel/sdk/metric"
	"go.opentelemetry.io/otel/sdk/resource"
	"go.opentelemetry.io/otel/sdk/trace"
	semconv "go.opentelemetry.io/otel/semconv/v1.4.0"
)

const (
	OtelMetricExportInterval = 1 * time.Minute
	OtelTraceExportInterval  = 1 * time.Minute
	OtelLogExportInterval    = 10 * time.Second
)

func newPropagator() propagation.TextMapPropagator {
	return propagation.NewCompositeTextMapPropagator(
		propagation.TraceContext{},
		propagation.Baggage{},
	)
}

func newTracerProvider(ctx context.Context, config OtelConfig, serviceResource *resource.Resource) (*trace.TracerProvider, error) {
	var exporter trace.SpanExporter
	var err error

	endpoint := config.TracesEndpoint
	if endpoint == "" {
		endpoint = config.ObservabilityEndpoint
	}

	switch config.Protocol {
	case OtelProtocolGRPC:
		exporter, err = otlptracegrpc.New(ctx, otlptracegrpc.WithEndpointURL(endpoint))
	case OtelProtocolHTTP:
		exporter, err = otlptracehttp.New(ctx, otlptracehttp.WithEndpointURL(endpoint))
	default:
		return nil, fmt.Errorf("unknown observability protocol '%s'", config.Protocol)
	}
	if err != nil {
		return nil, err
	}

	spanLimits := trace.SpanLimits{
		AttributePerEventCountLimit: 16,
		EventCountLimit:             64,
	}

	traceProvider := trace.NewTracerProvider(
		trace.WithResource(serviceResource),
		trace.WithBatcher(exporter,
			trace.WithBatchTimeout(OtelTraceExportInterval),
		),
		trace.WithRawSpanLimits(spanLimits),
		trace.WithSampler(trace.AlwaysSample()),
	)

	return traceProvider, nil
}

func newMeterProvider(ctx context.Context, config OtelConfig, serviceResource *resource.Resource) (*metric.MeterProvider, error) {
	var exporter metric.Exporter
	var err error

	endpoint := config.MetricsEndpoint
	if endpoint == "" {
		endpoint = config.ObservabilityEndpoint
	}

	switch config.Protocol {
	case OtelProtocolGRPC:
		exporter, err = otlpmetricgrpc.New(ctx, otlpmetricgrpc.WithEndpointURL(endpoint))
	case OtelProtocolHTTP:
		exporter, err = otlpmetrichttp.New(ctx, otlpmetrichttp.WithEndpointURL(endpoint))
	default:
		return nil, fmt.Errorf("unknown observability protocol '%s'", config.Protocol)
	}
	if err != nil {
		return nil, err
	}

	meterProvider := metric.NewMeterProvider(
		metric.WithResource(serviceResource),
		metric.WithReader(metric.NewPeriodicReader(exporter,
			metric.WithInterval(OtelMetricExportInterval))),
	)

	return meterProvider, nil
}

func newLoggerProvider(ctx context.Context, config OtelConfig, serviceResource *resource.Resource) (*log.LoggerProvider, error) {
	var exporter log.Exporter
	var err error

	endpoint := config.LogsEndpoint
	if endpoint == "" {
		endpoint = config.ObservabilityEndpoint
	}

	switch config.Protocol {
	case OtelProtocolGRPC:
		exporter, err = otlploggrpc.New(ctx, otlploggrpc.WithEndpointURL(endpoint))
	case OtelProtocolHTTP:
		exporter, err = otlploghttp.New(ctx, otlploghttp.WithEndpointURL(endpoint))
	default:
		return nil, fmt.Errorf("unknown observability protocol '%s'", config.Protocol)
	}
	if err != nil {
		return nil, err
	}

	loggerProvider := log.NewLoggerProvider(
		log.WithResource(serviceResource),
		log.WithProcessor(log.NewBatchProcessor(exporter,
			log.WithExportInterval(OtelLogExportInterval))),
	)

	return loggerProvider, nil
}

func newServiceResource(ctx context.Context, name string) (*resource.Resource, error) {
	providerBinary, err := os.Executable()
	if err != nil {
		return nil, err
	}
	serviceName := semconv.ServiceNameKey.String(filepath.Base(providerBinary))
	providerName := semconv.ServiceInstanceIDKey.String(name)
	return resource.New(ctx,
		resource.WithAttributes(
			serviceName,
			providerName,
		),
	)
}
