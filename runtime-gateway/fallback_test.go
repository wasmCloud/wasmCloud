package main

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"strings"
	"testing"
	"time"

	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/manager"
)

type recordingManager struct {
	ctrl.Manager
	runnable manager.Runnable
	err      error
}

func (m *recordingManager) Add(r manager.Runnable) error {
	m.runnable = r
	return m.err
}

func TestFallbackServerUnavailableAndShutdown(t *testing.T) {
	addr := freePort(t)
	fallback := &FallbackServer{BindAddr: addr}
	ctx, cancel := context.WithCancel(t.Context())
	t.Cleanup(cancel)
	done := make(chan error, 1)
	go func() { done <- fallback.Start(ctx) }()
	waitForListener(t, addr)

	resp, err := http.Get("http://" + addr + "/missing?query=value")
	if err != nil {
		t.Fatal(err)
	}
	body, err := io.ReadAll(resp.Body)
	_ = resp.Body.Close()
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != http.StatusServiceUnavailable || string(body) != "No backend available\n" || !resp.Close {
		t.Fatalf("fallback response: status=%d body=%q close=%v", resp.StatusCode, body, resp.Close)
	}
	cancel()
	select {
	case err := <-done:
		if !errors.Is(err, http.ErrServerClosed) {
			t.Fatalf("Start() = %v, want ErrServerClosed", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("fallback did not stop")
	}
	for _, lookup := range []func(string) (string, string){fallback.InvalidHostname, fallback.NoWorkloads} {
		if scheme, endpoint := lookup("missing.example"); scheme != "http" || endpoint != addr {
			t.Errorf("fallback route = %s://%s, want http://%s", scheme, endpoint, addr)
		}
	}
}

func TestServersRejectOccupiedAddress(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	addr := listener.Addr().String()
	for name, server := range map[string]manager.Runnable{
		"fallback": &FallbackServer{BindAddr: addr},
		"gateway":  &HTTPGateway{BindAddr: addr, Proxy: &httputil.ReverseProxy{}},
	} {
		t.Run(name, func(t *testing.T) {
			if err := server.Start(t.Context()); err == nil {
				t.Fatal("Start() accepted an occupied address")
			}
		})
	}
}

func TestExternalFallback(t *testing.T) {
	fallback := &ExternalFallback{Scheme: "https", Endpoint: "fallback.example:8443"}
	for _, lookup := range []func(string) (string, string){fallback.InvalidHostname, fallback.NoWorkloads} {
		if scheme, endpoint := lookup("missing.example"); scheme != "https" || endpoint != fallback.Endpoint {
			t.Errorf("external fallback = %s://%s", scheme, endpoint)
		}
	}
}

func TestFallbackSetupPropagatesManagerError(t *testing.T) {
	want := errors.New("manager stopped")
	for _, managerError := range []error{nil, want} {
		m := &recordingManager{err: managerError}
		fallback := &FallbackServer{}
		if err := fallback.SetupWithManager(t.Context(), m); !errors.Is(err, managerError) {
			t.Fatalf("SetupWithManager() = %v, want %v", err, managerError)
		}
		if m.runnable != fallback {
			t.Fatal("fallback was not registered with the manager")
		}
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestGatewayProxyResponses(t *testing.T) {
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Host != "public.example" || r.URL.RequestURI() != "/path?key=value" || r.Header.Get("X-Workload-Id") != "workload-id" {
			t.Errorf("forwarded request: host=%q uri=%q workload=%q", r.Host, r.URL.RequestURI(), r.Header.Get("X-Workload-Id"))
		}
		w.Header().Set("X-Backend", "reached")
		_, _ = io.WriteString(w, "backend response")
	}))
	t.Cleanup(backend.Close)
	for _, failure := range []bool{false, true} {
		t.Run(map[bool]string{false: "success", true: "transport failure"}[failure], func(t *testing.T) {
			proxy := &httputil.ReverseProxy{}
			if failure {
				proxy.Transport = roundTripFunc(func(*http.Request) (*http.Response, error) {
					return nil, errors.New("backend unavailable")
				})
			}
			gateway := &HTTPGateway{Proxy: proxy, Resolver: fakeResolver{LookupResult{
				Scheme: "http", Hostname: strings.TrimPrefix(backend.URL, "http://"), WorkloadID: "workload-id",
			}}}
			m := &recordingManager{}
			if err := gateway.SetupWithManager(t.Context(), m); err != nil || m.runnable != gateway {
				t.Fatalf("register gateway: %v", err)
			}
			response := httptest.NewRecorder()
			proxy.ServeHTTP(response, httptest.NewRequest(http.MethodGet, "http://public.example/path?key=value", nil))
			if failure {
				if response.Code != http.StatusBadGateway || response.Body.String() != "Gateway Error\n" || response.Header().Get("Connection") != "close" {
					t.Fatalf("proxy error response: %v", response)
				}
			} else if response.Code != http.StatusOK || response.Body.String() != "backend response" || response.Header().Get("X-Backend") != "reached" {
				t.Fatalf("proxy response: %v", response)
			}
		})
	}
	want := errors.New("manager stopped")
	gateway := &HTTPGateway{Proxy: &httputil.ReverseProxy{}}
	if err := gateway.SetupWithManager(t.Context(), &recordingManager{err: want}); !errors.Is(err, want) {
		t.Fatalf("SetupWithManager() = %v, want %v", err, want)
	}
}

func TestRewriteInvalidPeerAndConnection(t *testing.T) {
	gateway := &HTTPGateway{Resolver: fakeResolver{LookupResult{Scheme: "http", Hostname: "backend:8080", WorkloadID: "id"}}}
	request := newProxyRequest("invalid-peer", http.Header{"X-Real-Ip": {"spoofed"}, "Connection": {"upgrade"}})
	gateway.rewrite(request)
	if request.Out.Header.Get("X-Real-IP") != "" || request.Out.Header.Get("Connection") != "upgrade" || request.Out.Header.Get("X-Workload-Id") != "id" {
		t.Fatalf("rewritten headers: %v", request.Out.Header)
	}
}
