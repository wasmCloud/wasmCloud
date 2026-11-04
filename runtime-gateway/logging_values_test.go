package main

import (
	"bytes"
	"encoding/json"
	"io"
	"log/slog"
	"strings"
	"testing"

	"github.com/go-logr/logr"
	"k8s.io/apimachinery/pkg/types"
)

func TestGatewayKubernetesLogValues(t *testing.T) {
	key := types.NamespacedName{Namespace: "tenant", Name: "workload"}
	for _, tc := range []struct {
		name   string
		groups []string
		emit   func(*slog.Logger)
	}{
		{"controller fields", nil, func(logger *slog.Logger) {
			logr.FromSlogHandler(logger.Handler()).Info(testGatewayMessage, "workload", key)
		}},
		{"controller bound fields", nil, func(logger *slog.Logger) {
			logr.FromSlogHandler(logger.Handler()).WithValues("workload", key).Info(testGatewayMessage)
		}},
		{"slog bound fields", nil, func(logger *slog.Logger) {
			logger.With("workload", key).Info(testGatewayMessage)
		}},
		{"slog record group", []string{"payload"}, func(logger *slog.Logger) {
			logger.Info(testGatewayMessage, slog.Group("payload", slog.Any("workload", key)))
		}},
		{"slog bound group", []string{"payload"}, func(logger *slog.Logger) {
			logger.WithGroup("payload").With("workload", key).Info(testGatewayMessage)
		}},
		{"slog nested groups", []string{"payload", "inner"}, func(logger *slog.Logger) {
			logger.WithGroup("payload").WithGroup("inner").Info(testGatewayMessage, "workload", key)
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			options, err := parseGatewayOptions(nil, io.Discard)
			if err != nil {
				t.Fatal(err)
			}
			var output bytes.Buffer
			tc.emit(newGatewayLogger(options.logging, &output))
			var record map[string]any
			if err := json.Unmarshal(output.Bytes(), &record); err != nil {
				t.Fatal(err)
			}
			for _, name := range tc.groups {
				group, ok := record[name].(map[string]any)
				if !ok {
					t.Fatalf("group %s missing: %v", name, record)
				}
				record = group
			}
			value, ok := record["workload"].(map[string]any)
			if !ok || len(value) != 2 || value["name"] != key.Name || value["namespace"] != key.Namespace {
				t.Fatalf("workload log value = %v, want lowercase name and namespace", record["workload"])
			}
		})
	}
}

type panickingLogMarshaler struct{}

func (panickingLogMarshaler) MarshalLog() any { panic("test MarshalLog panic") }

type groupedLogMarshaler struct{}

func (groupedLogMarshaler) MarshalLog() any {
	return slog.GroupValue(slog.Any("object", types.NamespacedName{Namespace: "tenant", Name: "workload"}))
}

func TestGatewayLogMarshalerSafetyAndGroups(t *testing.T) {
	options, err := parseGatewayOptions(nil, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	var output bytes.Buffer
	logger := newGatewayLogger(options.logging, &output)
	logger.Info(testGatewayMessage, "broken", panickingLogMarshaler{}, "nested", groupedLogMarshaler{})
	var record map[string]any
	if err := json.Unmarshal(output.Bytes(), &record); err != nil {
		t.Fatal(err)
	}
	failure, ok := record["broken"].(string)
	if !ok || !strings.Contains(strings.ToLower(failure), "panic") || record["msg"] != testGatewayMessage {
		t.Fatalf("marshaler panic was not safely logged: %v", record)
	}
	nested, ok := record["nested"].(map[string]any)
	if !ok {
		t.Fatalf("marshaler group missing: %v", record)
	}
	object, ok := nested["object"].(map[string]any)
	if !ok || object["name"] != "workload" || object["namespace"] != "tenant" {
		t.Fatalf("marshaler group value = %v", nested)
	}
}
