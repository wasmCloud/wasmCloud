package main

import (
	"bytes"
	"errors"
	"flag"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const (
	testGatewayBind    = "127.0.0.1:9080"
	testFallbackOrigin = "https://fallback.example:8443"
	testKubernetesHost = "https://kubernetes.example"
	testGatewayMessage = "gateway log message"
)

func TestGatewayFlagsAndLogging(t *testing.T) {
	for _, tc := range []struct {
		name  string
		args  []string
		level slog.Level
	}{
		{"defaults", nil, slog.LevelInfo},
		{"development", []string{"--dev-mode"}, slog.LevelDebug},
		{"disabled development", []string{"--dev-mode=false"}, slog.LevelInfo},
		{"explicit debug", []string{"--log-level=debug"}, slog.LevelDebug},
		{"explicit warning", []string{"--log-level=warn"}, slog.LevelWarn},
		{"explicit error", []string{"--log-level=error"}, slog.LevelError},
		{"level before development", []string{"--log-level=info", "--dev-mode"}, slog.LevelInfo},
		{"level after development", []string{"--dev-mode", "--log-level=error"}, slog.LevelError},
	} {
		t.Run(tc.name, func(t *testing.T) {
			args := append([]string{"--bind-addr=" + testGatewayBind, "--fallback-endpoint=" + testFallbackOrigin,
				"--kubeconfig=/test/kubeconfig"}, tc.args...)
			options, err := parseGatewayOptions(args, io.Discard)
			if err != nil {
				t.Fatal(err)
			}
			if options.bindAddr != testGatewayBind || options.fallbackEndpoint != testFallbackOrigin ||
				options.kubeconfig != "/test/kubeconfig" || options.logging.level != tc.level {
				t.Fatalf("parsed options = %+v", options)
			}
			logger := newGatewayLogger(options.logging, io.Discard)
			for _, level := range []slog.Level{slog.LevelDebug, slog.LevelInfo, slog.LevelWarn, slog.LevelError} {
				if logger.Enabled(t.Context(), level) != (level >= tc.level) {
					t.Fatalf("level %s enabled = %v", level, logger.Enabled(t.Context(), level))
				}
			}
		})
	}
	options, err := parseGatewayOptions(nil, io.Discard)
	if err != nil || options.bindAddr != ":8000" || options.fallbackEndpoint != "" || options.kubeconfig != "" ||
		options.logging.format != "json" || options.logging.timeEncoding != "epoch" {
		t.Fatalf("default options = %+v, %v", options, err)
	}
}

func TestGatewayFlagErrorsAndHelp(t *testing.T) {
	for _, arg := range []string{"--unknown", "--dev-mode=invalid", "--log-format=invalid", "--log-level=invalid",
		"--log-time-encoding=invalid", "--log-level=error+1", "--zap-devel"} {
		var output bytes.Buffer
		if _, err := parseGatewayOptions([]string{arg}, &output); err == nil || output.Len() == 0 {
			t.Fatalf("parse %q = %v, output=%q", arg, err, output.String())
		}
	}
	var output bytes.Buffer
	if _, err := parseGatewayOptions([]string{"--help"}, &output); !errors.Is(err, flag.ErrHelp) {
		t.Fatalf("help = %v", err)
	}
	for _, name := range []string{"bind-addr", "fallback-endpoint", "kubeconfig", "dev-mode", "log-level", "log-format", "log-time-encoding"} {
		if !strings.Contains(output.String(), name) {
			t.Errorf("help omitted %s", name)
		}
	}
}

func writeGatewayKubeconfig(t *testing.T) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "config")
	data := `apiVersion: v1
kind: Config
clusters:
- name: test
  cluster:
    server: ` + testKubernetesHost + `
contexts:
- name: test
  context:
    cluster: test
    user: test
current-context: test
users:
- name: test
  user:
    token: test-token
`
	if err := os.WriteFile(path, []byte(data), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestGatewayConfigLoading(t *testing.T) {
	path := writeGatewayKubeconfig(t)
	t.Setenv("KUBECONFIG", path)
	t.Setenv("KUBERNETES_SERVICE_HOST", "")
	for _, explicit := range []string{"", path} {
		config, err := loadGatewayConfig(explicit)
		if err != nil {
			t.Fatal(err)
		}
		if config.Host != testKubernetesHost || config.BearerToken != "test-token" || config.QPS != -1 {
			t.Fatalf("loaded config host=%q token=%q qps=%v", config.Host, config.BearerToken, config.QPS)
		}
	}
	missing := filepath.Join(t.TempDir(), "missing")
	if _, err := loadGatewayConfig(missing); err == nil {
		t.Fatal("explicit missing kubeconfig fell back to KUBECONFIG")
	}
	t.Setenv("KUBECONFIG", missing)
	if config, err := loadGatewayConfig(path); err != nil || config.Host != testKubernetesHost {
		t.Fatalf("explicit kubeconfig did not take precedence: %v", err)
	}
}

func TestExternalFallbackOrigins(t *testing.T) {
	for _, tc := range []struct {
		endpoint string
		scheme   string
		host     string
	}{
		{"http://fallback.example", "http", "fallback.example"},
		{testFallbackOrigin + "/", "https", "fallback.example:8443"},
		{"HTTP://[::1]:8080", "http", "[::1]:8080"},
	} {
		fallback, err := externalFallback(tc.endpoint)
		if err != nil {
			t.Fatal(err)
		}
		if scheme, host := fallback.NoWorkloads("route.example"); scheme != tc.scheme || host != tc.host {
			t.Fatalf("fallback for %q = %s://%s", tc.endpoint, scheme, host)
		}
	}
	for _, endpoint := range []string{
		"%", "fallback.example", "//fallback.example", "http:///missing-host", "http://:80", "ftp://fallback.example",
		"https://user:password@fallback.example", testFallbackOrigin + "/path", testFallbackOrigin + "?key=value",
		testFallbackOrigin + "?", testFallbackOrigin + "#fragment",
	} {
		if _, err := externalFallback(endpoint); err == nil {
			t.Errorf("accepted invalid fallback origin %q", endpoint)
		}
	}
}
