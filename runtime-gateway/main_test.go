package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestGatewayMainProcess(t *testing.T) {
	if os.Getenv("WASMCLOUD_GATEWAY_MAIN_TEST") != "1" {
		return
	}
	separator := slices.Index(os.Args, "--")
	if separator < 0 {
		t.Fatal("missing gateway arguments")
	}
	os.Args = append([]string{"runtime-gateway"}, os.Args[separator+1:]...)
	main()
}

func TestGatewayMainExitCodes(t *testing.T) {
	missingConfig := filepath.Join(t.TempDir(), "missing-config")
	for _, tc := range []struct {
		name string
		args []string
		code int
		text string
	}{
		{"help", []string{"--help"}, 0, "-log-level"},
		{"unknown flag", []string{"--unknown"}, 2, "flag provided but not defined"},
		{"invalid logging", []string{"--log-level=invalid"}, 2, "invalid value"},
		{"invalid log format", []string{"--log-format=invalid"}, 2, "invalid log format"},
		{"invalid log timestamp", []string{"--log-time-encoding=invalid"}, 2, "invalid log time encoding"},
		{"missing kubeconfig", []string{"--log-level=debug", "--kubeconfig=" + missingConfig}, 1, "get kubeconfig"},
		{"invalid fallback", []string{"--fallback-endpoint=ftp://example.com", "--kubeconfig=" + missingConfig},
			1, "fallback endpoint must be an absolute HTTP or HTTPS URL"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, os.Args[0], append([]string{"-test.run=^TestGatewayMainProcess$", "--"}, tc.args...)...)
			cmd.Env = append(os.Environ(), "WASMCLOUD_GATEWAY_MAIN_TEST=1")
			output, err := cmd.CombinedOutput()
			code := 0
			if err != nil {
				var exitError *exec.ExitError
				if !errors.As(err, &exitError) {
					t.Fatalf("run gateway: %v", err)
				}
				code = exitError.ExitCode()
			}
			if code != tc.code || !strings.Contains(string(output), tc.text) {
				t.Fatalf("exit=%d output=%s; want exit=%d containing %q", code, output, tc.code, tc.text)
			}
			if tc.code == 1 {
				line, _, _ := strings.Cut(string(output), "\n")
				var record map[string]any
				if err := json.Unmarshal([]byte(line), &record); err != nil {
					t.Fatalf("startup error is not JSON: %s: %v", output, err)
				}
				message, ok := record["error"].(string)
				if !ok || !strings.Contains(message, tc.text) || record["logger"] != "setup" || record["level"] != "error" {
					t.Fatalf("startup error fields = %v", record)
				}
				if timestamp, ok := record["ts"].(float64); !ok || timestamp <= 0 {
					t.Fatalf("startup timestamp = %v, want numeric epoch seconds", record["ts"])
				}
			}
		})
	}
}
