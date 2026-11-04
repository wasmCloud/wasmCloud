package main

import (
	"bytes"
	"encoding/json"
	"log/slog"
	"strings"
	"testing"

	"github.com/go-logr/logr"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
)

const (
	testTenant   = "tenant"
	testPayload  = "payload"
	testWorkload = "workload"
)

func TestOperatorKubernetesLogValues(t *testing.T) {
	key := types.NamespacedName{Namespace: testTenant, Name: testWorkload}
	for _, tc := range []struct {
		name   string
		groups []string
		emit   func(*slog.Logger)
	}{
		{"controller fields", nil, func(logger *slog.Logger) {
			logr.FromSlogHandler(logger.Handler()).Info(testLogMessage, testWorkload, key)
		}},
		{"controller bound fields", nil, func(logger *slog.Logger) {
			logr.FromSlogHandler(logger.Handler()).WithValues(testWorkload, key).Info(testLogMessage)
		}},
		{"slog bound fields", nil, func(logger *slog.Logger) {
			logger.With(testWorkload, key).Info(testLogMessage)
		}},
		{"slog record group", []string{testPayload}, func(logger *slog.Logger) {
			logger.Info(testLogMessage, slog.Group(testPayload, slog.Any(testWorkload, key)))
		}},
		{"slog bound group", []string{testPayload}, func(logger *slog.Logger) {
			logger.WithGroup(testPayload).With(testWorkload, key).Info(testLogMessage)
		}},
		{"slog nested groups", []string{testPayload, "inner"}, func(logger *slog.Logger) {
			logger.WithGroup(testPayload).WithGroup("inner").Info(testLogMessage, testWorkload, key)
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			options := parseLogging(t, testJSONFlag)
			var output bytes.Buffer
			logger, err := options.logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			tc.emit(logger)
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
			value, ok := record[testWorkload].(map[string]any)
			if !ok || len(value) != 2 || value["name"] != key.Name || value["namespace"] != key.Namespace {
				t.Fatalf("workload log value = %v, want lowercase name and namespace", record[testWorkload])
			}
		})
	}
}

type panickingLogMarshaler struct{}

func (panickingLogMarshaler) MarshalLog() any { panic("test MarshalLog panic") }

type groupedLogMarshaler struct{}

func (groupedLogMarshaler) MarshalLog() any {
	return slog.GroupValue(slog.Any("object", types.NamespacedName{Namespace: testTenant, Name: testWorkload}))
}

func TestOperatorLogMarshalerSafetyAndGroups(t *testing.T) {
	options := parseLogging(t, testJSONFlag)
	var output bytes.Buffer
	logger, err := options.logger(&output)
	if err != nil {
		t.Fatal(err)
	}
	logger.Info(testLogMessage, "broken", panickingLogMarshaler{}, "nested", groupedLogMarshaler{})
	var record map[string]any
	if err := json.Unmarshal(output.Bytes(), &record); err != nil {
		t.Fatal(err)
	}
	failure, ok := record["broken"].(string)
	if !ok || !strings.Contains(strings.ToLower(failure), "panic") || record["msg"] != testLogMessage {
		t.Fatalf("marshaler panic was not safely logged: %v", record)
	}
	nested, ok := record["nested"].(map[string]any)
	if !ok {
		t.Fatalf("marshaler group missing: %v", record)
	}
	object, ok := nested["object"].(map[string]any)
	if !ok || object["name"] != testWorkload || object["namespace"] != testTenant {
		t.Fatalf("marshaler group value = %v", nested)
	}
}

type objectLogMarshaler struct{ object any }

func (value objectLogMarshaler) MarshalLog() any { return value.object }

func TestProductionObjectLogging(t *testing.T) {
	secret := &corev1.Secret{
		TypeMeta:   metav1.TypeMeta{APIVersion: "v1", Kind: "Secret"},
		ObjectMeta: metav1.ObjectMeta{Name: "credentials", Namespace: testTenant},
		StringData: map[string]string{"password": "private-test-value"},
	}
	for _, value := range []struct {
		name   string
		object any
	}{
		{"direct", secret},
		{"marshaled", objectLogMarshaler{secret}},
	} {
		for _, tc := range []struct {
			name string
			emit func(*slog.Logger)
		}{
			{"inline", func(logger *slog.Logger) { logger.Info(testLogMessage, "object", value.object) }},
			{"bound", func(logger *slog.Logger) { logger.With("object", value.object).Info(testLogMessage) }},
			{"grouped", func(logger *slog.Logger) {
				logger.WithGroup(testPayload).Info(testLogMessage, "object", value.object)
			}},
			{"controller inline", func(logger *slog.Logger) {
				logr.FromSlogHandler(controllerLogHandler{logger.Handler()}).Info(testLogMessage, "object", value.object)
			}},
			{"controller bound", func(logger *slog.Logger) {
				logr.FromSlogHandler(controllerLogHandler{logger.Handler()}).
					WithValues("object", value.object).Info(testLogMessage)
			}},
		} {
			t.Run(value.name+"/"+tc.name, func(t *testing.T) {
				var output bytes.Buffer
				logger, err := parseLogging(t, testLegacyProductionFlag).logger(&output)
				if err != nil {
					t.Fatal(err)
				}
				tc.emit(logger)
				if strings.Contains(output.String(), "private-test-value") || strings.Contains(output.String(), "stringData") {
					t.Fatalf("object contents logged: %s", output.String())
				}
				record := decodeLog(t, output.Bytes())
				if tc.name == "grouped" {
					record = record[testPayload].(map[string]any)
				}
				object, ok := record["object"].(map[string]any)
				if !ok || len(object) != 4 || object["name"] != secret.Name || object["namespace"] != secret.Namespace ||
					object["kind"] != "Secret" || object["apiVersion"] != "v1" {
					t.Fatalf("object identity = %v", object)
				}
			})
		}
	}
}
