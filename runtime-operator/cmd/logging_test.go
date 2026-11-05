package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const (
	logError                 = "error"
	testLegacyProductionFlag = "--zap-devel=false"
	testJSONFlag             = "--json-log"
	testJSONFormatFlag       = "--log-format=json"
)

func decodeLog(t *testing.T, data []byte) map[string]any {
	t.Helper()
	var record map[string]any
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	if err := decoder.Decode(&record); err != nil {
		t.Fatalf("decode log: %v: %s", err, data)
	}
	return record
}

func TestOperatorLoggingSubprocess(t *testing.T) {
	if os.Getenv("OPERATOR_LOGGING_HELPER") == "1" {
		for i, arg := range os.Args {
			if arg == "--" {
				os.Args = append([]string{os.Args[0]}, os.Args[i+1:]...)
				break
			}
		}
		main()
		os.Exit(0)
	}
	for _, args := range [][]string{
		{testJSONFlag, "--log-level=error"},
		{testLegacyProductionFlag, "--zap-log-level=error", "--zap-time-encoding=epoch"},
		{testJSONFormatFlag, "--log-stacktrace-level=panic"},
		{"--log-format=bad"},
	} {
		t.Run(strings.Join(args, " "), func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			config := filepath.Join(t.TempDir(), "invalid-kubeconfig")
			if err := os.WriteFile(config, []byte("apiVersion: [invalid"), 0600); err != nil {
				t.Fatal(err)
			}
			commandArgs := append([]string{"-test.run=^TestOperatorLoggingSubprocess$", "--",
				"--nats-url=nats://127.0.0.1:1"}, args...)
			command := exec.CommandContext(ctx, os.Args[0], commandArgs...)
			command.Env = append(os.Environ(), "OPERATOR_LOGGING_HELPER=1", "OPERATOR_NAMESPACE=", "KUBECONFIG="+config)
			output, err := command.CombinedOutput()
			if ctx.Err() != nil {
				t.Fatalf("startup did not reject missing namespace before timeout: %v: %s", ctx.Err(), output)
			}
			var exit *exec.ExitError
			if !errors.As(err, &exit) {
				t.Fatalf("process = %v: %s", err, output)
			}
			if args[0] == "--log-format=bad" {
				if exit.ExitCode() != 2 || !strings.Contains(string(output), "invalid log format") {
					t.Fatalf("invalid flags: %v: %s", err, output)
				}
				return
			}
			if exit.ExitCode() != 1 {
				t.Fatalf("exit = %d: %s", exit.ExitCode(), output)
			}
			record := decodeLog(t, output)
			if record["level"] != logError || record["logger"] != "setup" ||
				record[logError] != "missing required configuration: OPERATOR_NAMESPACE is unset" || record["ts"] == nil {
				t.Fatalf("startup error = %v", record)
			}
			if args[0] == testJSONFlag {
				if _, ok := record["ts"].(json.Number); !ok {
					t.Fatalf("epoch timestamp = %v", record["ts"])
				}
			}
		})
	}
}
