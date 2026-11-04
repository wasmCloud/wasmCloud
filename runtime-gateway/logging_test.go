package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/go-logr/logr"
)

func TestGatewayLogFormatsAndTimestamps(t *testing.T) {
	timestamp := time.Unix(1234567890, 250000000).UTC()
	for _, format := range []string{"json", "text"} {
		for _, encoding := range []string{"epoch", "rfc3339", "rfc3339nano"} {
			t.Run(format+"/"+encoding, func(t *testing.T) {
				options, err := parseGatewayOptions([]string{"--log-format=" + format, "--log-time-encoding=" + encoding}, io.Discard)
				if err != nil {
					t.Fatal(err)
				}
				var output bytes.Buffer
				logger := newGatewayLogger(options.logging, &output)
				record := slog.NewRecord(timestamp, slog.LevelInfo, testGatewayMessage, 0)
				if err := logger.Handler().Handle(t.Context(), record); err != nil {
					t.Fatal(err)
				}
				var want any = json.Number("1234567890.25")
				switch encoding {
				case "rfc3339":
					want = timestamp.Format(time.RFC3339)
				case "rfc3339nano":
					want = timestamp.Format(time.RFC3339Nano)
				}
				if format == "json" {
					var fields map[string]any
					decoder := json.NewDecoder(&output)
					decoder.UseNumber()
					if err := decoder.Decode(&fields); err != nil {
						t.Fatal(err)
					}
					if fields["ts"] != want || fields["level"] != "info" || fields["msg"] != testGatewayMessage {
						t.Fatalf("JSON log = %v, want timestamp %v", fields, want)
					}
					if _, exists := fields[slog.TimeKey]; exists {
						t.Fatal("JSON log contains a second timestamp field")
					}
				} else {
					first, _, _ := strings.Cut(output.String(), " ")
					actual, ok := strings.CutPrefix(first, "ts=")
					if !ok {
						t.Fatalf("text timestamp missing: %q", output.String())
					}
					if value, ok := want.(string); ok {
						if actual != value {
							t.Fatalf("text timestamp = %q, want %q", actual, value)
						}
					} else if value, err := strconv.ParseFloat(actual, 64); err != nil || value != 1234567890.25 {
						t.Fatalf("text timestamp = %q, want numeric epoch seconds: %v", actual, err)
					}
					if json.Valid(output.Bytes()) ||
						!strings.Contains(output.String(), "level=info") || !strings.Contains(output.String(), testGatewayMessage) {
						t.Fatalf("text log = %q", output.String())
					}
				}
			})
		}
	}
}

func TestGatewayDefaultLogTimestamps(t *testing.T) {
	for _, args := range [][]string{nil, {"--dev-mode"}} {
		options, err := parseGatewayOptions(args, io.Discard)
		if err != nil {
			t.Fatal(err)
		}
		var output bytes.Buffer
		logger := newGatewayLogger(options.logging, &output)
		if err := logger.Handler().Handle(t.Context(), slog.NewRecord(time.Unix(1234567890, 250000000),
			slog.LevelInfo, testGatewayMessage, 0)); err != nil {
			t.Fatal(err)
		}
		var fields map[string]any
		if err := json.Unmarshal(output.Bytes(), &fields); err != nil {
			t.Fatal(err)
		}
		if fields["ts"] != 1234567890.25 {
			t.Fatalf("default timestamp with %v = %v, want numeric epoch seconds", args, fields["ts"])
		}
	}
}

func TestGatewayControllerLoggingBridge(t *testing.T) {
	options, err := parseGatewayOptions([]string{"--dev-mode"}, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	var output bytes.Buffer
	logger := newGatewayLogger(options.logging, &output)
	controller := logr.FromSlogHandler(logger.Handler()).WithName("gateway").WithValues("namespace", "tenant")
	controller.V(1).Info("controller detail")
	controller.V(4).Info("controller debug")
	controller.V(5).Info("filtered detail")
	controller.Info("controller info")
	logger.Warn("slog warning")
	wantError := errors.New("backend missing")
	controller.Error(wantError, "controller error")
	decoder := json.NewDecoder(&output)
	for _, want := range []struct{ message, level string }{
		{"controller detail", "debug"}, {"controller debug", "debug"},
		{"controller info", "info"}, {"slog warning", "warn"}, {"controller error", "error"},
	} {
		var fields map[string]any
		if err := decoder.Decode(&fields); err != nil {
			t.Fatal(err)
		}
		if fields["msg"] != want.message || fields["level"] != want.level {
			t.Fatalf("log fields = %v, want %+v", fields, want)
		}
		if strings.HasPrefix(want.message, "controller") && (fields["logger"] != "gateway" || fields["namespace"] != "tenant") {
			t.Fatalf("controller lost its name or fields: %v", fields)
		}
		if want.level == "error" {
			if fields["error"] != wantError.Error() {
				t.Fatalf("controller error = %v", fields)
			}
			if _, exists := fields["err"]; exists {
				t.Fatal("controller error has the wrong field name")
			}
		}
	}
	if err := decoder.Decode(new(any)); !errors.Is(err, io.EOF) {
		t.Fatalf("extra log record or invalid JSON: %v", err)
	}
}

func TestGatewayLoggingPreservesApplicationFields(t *testing.T) {
	options, err := parseGatewayOptions(nil, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	var output bytes.Buffer
	logger := newGatewayLogger(options.logging, &output)
	timestamp := time.Unix(1234567890, 250000000).UTC()
	logger.With("time", "application time").WithGroup("payload").With("err", "application error").Info(testGatewayMessage,
		slog.Time("time", timestamp), slog.String("level", "application level"))
	var fields map[string]any
	if err := json.Unmarshal(output.Bytes(), &fields); err != nil {
		t.Fatal(err)
	}
	payload, ok := fields["payload"].(map[string]any)
	if !ok || payload["time"] != timestamp.Format(time.RFC3339Nano) || payload["level"] != "application level" ||
		payload["err"] != "application error" || fields["time"] != "application time" {
		t.Fatalf("application fields changed: %v", fields)
	}
}
