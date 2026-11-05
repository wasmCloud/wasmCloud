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
		StringData: map[string]string{testPasswordKey: "private-test-value"},
	}
	for _, mode := range []struct {
		name string
		args []string
	}{
		{"default", []string{testJSONFormatFlag}},
		{"default text", nil},
		{"production", []string{testLegacyProductionFlag}},
	} {
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
				{testGrouped, func(logger *slog.Logger) {
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
				t.Run(mode.name+"/"+value.name+"/"+tc.name, func(t *testing.T) {
					var output bytes.Buffer
					logger, err := parseLogging(t, mode.args...).logger(&output)
					if err != nil {
						t.Fatal(err)
					}
					tc.emit(logger)
					if strings.Contains(output.String(), "private-test-value") || strings.Contains(output.String(), "stringData") {
						t.Fatalf("object contents logged: %s", output.String())
					}
					if mode.name == "default text" {
						if !strings.Contains(output.String(), "object.name=credentials") ||
							!strings.Contains(output.String(), "object.namespace="+testTenant) {
							t.Fatalf("text object identity = %s", output.String())
						}
						return
					}
					record := decodeLog(t, output.Bytes())
					if tc.name == testGrouped {
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
}

func TestNilObjectLogging(t *testing.T) {
	var secret *corev1.Secret
	for _, mode := range []struct {
		args []string
		want string
	}{
		{[]string{testJSONFormatFlag}, `"object":null`},
		{nil, "object=<nil>"},
	} {
		for _, value := range []any{secret, objectLogMarshaler{secret}} {
			var output bytes.Buffer
			logger, err := parseLogging(t, mode.args...).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			logger.Info(testLogMessage, "object", value)
			if !strings.Contains(output.String(), mode.want) || strings.Contains(output.String(), "panic") {
				t.Fatalf("nil object log = %s", output.String())
			}
		}
	}
}

func TestFullObjectLoggingRequiresOptIn(t *testing.T) {
	secret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: "credentials"},
		StringData: map[string]string{testPasswordKey: "opt-in-secret"},
	}
	for _, mode := range [][]string{{testJSONFormatFlag}, {testLegacyProductionFlag}} {
		for _, value := range []any{secret, objectLogMarshaler{secret}} {
			var output bytes.Buffer
			args := append(append([]string(nil), mode...), "--log-full-objects")
			logger, err := parseLogging(t, args...).logger(&output)
			if err != nil {
				t.Fatal(err)
			}
			logger.Info(testLogMessage, "object", value)
			if !strings.Contains(output.String(), "opt-in-secret") {
				t.Fatalf("full objects were not explicitly enabled: %s", output.String())
			}
		}
	}
}

func TestObjectListLoggingOmitsContents(t *testing.T) {
	list := &corev1.SecretList{
		TypeMeta: metav1.TypeMeta{APIVersion: "v1", Kind: "SecretList"},
		Items:    []corev1.Secret{{StringData: map[string]string{testPasswordKey: "list-secret"}}},
	}
	for _, value := range []any{list, objectLogMarshaler{list}} {
		var output bytes.Buffer
		logger, err := parseLogging(t, testJSONFormatFlag).logger(&output)
		if err != nil {
			t.Fatal(err)
		}
		logger.Info(testLogMessage, "object", value)
		record := decodeLog(t, output.Bytes())
		object, ok := record["object"].(map[string]any)
		if !ok || len(object) != 2 || object["kind"] != "SecretList" || strings.Contains(output.String(), "list-secret") {
			t.Fatalf("object list log = %v", record)
		}
	}
}
