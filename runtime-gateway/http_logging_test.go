package main

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"log"
	"log/slog"
	"net"
	"net/http"
	"net/http/httputil"
	"strings"
	"sync"
	"testing"
	"time"
)

type synchronizedLogBuffer struct {
	mu     sync.Mutex
	buffer bytes.Buffer
}

func (b *synchronizedLogBuffer) Write(data []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buffer.Write(data)
}

func (b *synchronizedLogBuffer) snapshot() []byte {
	b.mu.Lock()
	defer b.mu.Unlock()
	return bytes.Clone(b.buffer.Bytes())
}

func captureGatewayDefaultLogs(t *testing.T, level string) *synchronizedLogBuffer {
	t.Helper()
	options, err := parseGatewayOptions([]string{"--log-level=" + level}, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	oldLogger, oldWriter, oldFlags := slog.Default(), log.Writer(), log.Flags()
	output := &synchronizedLogBuffer{}
	slog.SetDefault(newGatewayLogger(options.logging, output))
	t.Cleanup(func() {
		slog.SetDefault(oldLogger)
		log.SetOutput(oldWriter)
		log.SetFlags(oldFlags)
	})
	return output
}

type temporaryAcceptError struct{}

func (temporaryAcceptError) Error() string   { return "test temporary accept failure" }
func (temporaryAcceptError) Temporary() bool { return true }
func (temporaryAcceptError) Timeout() bool   { return false }

type failingFirstAcceptListener struct {
	net.Listener
	once sync.Once
}

func (l *failingFirstAcceptListener) Accept() (net.Conn, error) {
	failed := false
	l.once.Do(func() { failed = true })
	if failed {
		return nil, temporaryAcceptError{}
	}
	return l.Listener.Accept()
}

func TestGatewayHTTPDiagnosticsAtAllLogLevels(t *testing.T) {
	for _, level := range []string{"info", "warn", "error"} {
		for _, kind := range []string{"http-gateway", "fallback"} {
			t.Run(kind+"/"+level, func(t *testing.T) {
				output := captureGatewayDefaultLogs(t, level)
				var start func(context.Context) error
				var addr, diagnostic string
				if kind == "http-gateway" {
					gateway := &HTTPGateway{BindAddr: freePort(t), Proxy: &httputil.ReverseProxy{
						Rewrite: func(*httputil.ProxyRequest) { panic("test request panic") },
					}}
					start, addr, diagnostic = gateway.Start, gateway.BindAddr, "http: panic serving"
				} else {
					fallback, err := newFallbackServer("127.0.0.1:0")
					if err != nil {
						t.Fatal(err)
					}
					t.Cleanup(func() { _ = fallback.Close() })
					fallback.listener = &failingFirstAcceptListener{Listener: fallback.listener}
					start, addr, diagnostic = fallback.Start, fallback.listener.Addr().String(), "http: Accept error:"
				}
				ctx, cancel := context.WithCancel(t.Context())
				done := make(chan error, 1)
				go func() { done <- start(ctx) }()
				var stopOnce sync.Once
				stop := func() {
					stopOnce.Do(func() {
						cancel()
						if err := awaitGatewayShutdown(t, done); err != nil {
							t.Errorf("HTTP server shutdown: %v", err)
						}
					})
				}
				t.Cleanup(stop)
				waitForListener(t, addr)
				client := &http.Client{Timeout: 5 * time.Second}
				response, err := client.Get("http://" + addr + "/")
				if response != nil {
					_, _ = io.Copy(io.Discard, response.Body)
					_ = response.Body.Close()
				}
				if kind == "http-gateway" && err == nil {
					t.Fatal("request panic did not close its connection")
				}
				if kind == "fallback" && (err != nil || response.StatusCode != http.StatusServiceUnavailable) {
					t.Fatalf("fallback did not recover from its accept error: %v", err)
				}
				stop()
				var record map[string]any
				if err := json.Unmarshal(output.snapshot(), &record); err != nil {
					t.Fatalf("missing or invalid HTTP diagnostic: %s: %v", output.snapshot(), err)
				}
				message, ok := record["msg"].(string)
				if !ok || !strings.Contains(message, diagnostic) || record["level"] != "error" || record["logger"] != kind {
					t.Fatalf("HTTP diagnostic = %v", record)
				}
			})
		}
	}
}
