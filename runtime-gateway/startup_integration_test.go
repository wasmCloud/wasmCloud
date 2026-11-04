package main

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/manager"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func runGatewayServers(ctx context.Context, runnables []manager.Runnable) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	results := make(chan error, len(runnables))
	for _, runnable := range runnables {
		go func() { results <- runnable.Start(ctx) }()
	}
	var result error
	for range runnables {
		if err := <-results; err != nil {
			result = errors.Join(result, err)
			cancel()
		}
	}
	return result
}

func TestGatewayStartupServesRoutesAndStops(t *testing.T) {
	for _, external := range []bool{false, true} {
		t.Run(map[bool]string{false: "builtin", true: "external"}[external], func(t *testing.T) {
			backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.Host != "a.localhost.direct" || r.URL.RequestURI() != "/path?key=value" ||
					r.Header.Get("X-Workload-Id") != "workload-id" {
					t.Errorf("forwarded request: host=%q uri=%q headers=%v", r.Host, r.URL.RequestURI(), r.Header)
				}
				_, _ = io.WriteString(w, "routed workload")
			}))
			t.Cleanup(backend.Close)
			options, deps, m := gatewayStartupFixture(t)
			options.bindAddr = freePort(t)
			if external {
				fallback := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					if r.Host != "unknown.example" || r.URL.RequestURI() != "/path?key=value" {
						t.Errorf("fallback request: host=%q uri=%q", r.Host, r.URL.RequestURI())
					}
					w.WriteHeader(http.StatusNotFound)
					_, _ = io.WriteString(w, "external fallback")
				}))
				t.Cleanup(fallback.Close)
				options.fallbackEndpoint = fallback.URL
			}
			hostname, port, err := net.SplitHostPort(strings.TrimPrefix(backend.URL, "http://"))
			if err != nil {
				t.Fatal(err)
			}
			httpPort, err := strconv.ParseUint(port, 10, 16)
			if err != nil {
				t.Fatal(err)
			}
			host := &runtimev1alpha1.Host{
				ObjectMeta: metav1.ObjectMeta{Namespace: "ns", Name: "host"},
				HostID:     "host-id", Hostname: hostname, HTTPPort: uint32(httpPort),
			}
			host.Status.SetConditions(condition.Condition{Type: condition.TypeReady, Status: condition.ConditionTrue})
			workload := availableWorkload("workload")
			m.client = fake.NewClientBuilder().WithScheme(scheme).WithObjects(host, workload).Build()
			setupHost, setupWorkload := deps.setupHost, deps.setupWorkload
			deps.setupHost = func(r *HostReconciler, ctx context.Context, mgr ctrl.Manager) error {
				if err := setupHost(r, ctx, mgr); err != nil {
					return err
				}
				_, err := r.Reconcile(ctx, ctrl.Request{NamespacedName: client.ObjectKeyFromObject(host)})
				return err
			}
			deps.setupWorkload = func(r *WorkloadReconciler, ctx context.Context, mgr ctrl.Manager) error {
				if err := setupWorkload(r, ctx, mgr); err != nil {
					return err
				}
				_, err := r.Reconcile(ctx, ctrl.Request{NamespacedName: client.ObjectKeyFromObject(workload)})
				return err
			}
			m.start = func(ctx context.Context) error { return runGatewayServers(ctx, m.runnables) }
			ctx, cancel := context.WithCancel(t.Context())
			done := make(chan error, 1)
			go func() { done <- runGateway(ctx, options, deps) }()
			t.Cleanup(func() {
				cancel()
				if done != nil {
					_ = awaitGatewayShutdown(t, done)
				}
			})
			waitForListener(t, options.bindAddr)
			httpClient := &http.Client{Timeout: 5 * time.Second}
			for _, route := range []struct {
				host   string
				status int
				body   string
			}{
				{"a.localhost.direct", http.StatusOK, "routed workload"},
				{"unknown.example", map[bool]int{false: http.StatusServiceUnavailable, true: http.StatusNotFound}[external],
					map[bool]string{false: "No backend available\n", true: "external fallback"}[external]},
			} {
				request, err := http.NewRequestWithContext(t.Context(), http.MethodGet, "http://"+options.bindAddr+"/path?key=value", nil)
				if err != nil {
					t.Fatal(err)
				}
				request.Host = route.host
				resp, err := httpClient.Do(request)
				if err != nil {
					t.Fatal(err)
				}
				body, err := io.ReadAll(resp.Body)
				_ = resp.Body.Close()
				if err != nil || resp.StatusCode != route.status || string(body) != route.body {
					t.Fatalf("%s response: status=%d body=%q err=%v", route.host, resp.StatusCode, body, err)
				}
			}
			cancel()
			err = awaitGatewayShutdown(t, done)
			done = nil
			if err != nil {
				t.Fatal(err)
			}
			assertGatewayListenerClosed(t, options.bindAddr)
			assertGatewaySetupCleanedUp(t, m)
		})
	}
}

func TestGatewayStartupBindFailureStopsFallback(t *testing.T) {
	occupied, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = occupied.Close() })
	options, deps, m := gatewayStartupFixture(t)
	options.bindAddr = occupied.Addr().String()
	m.start = func(ctx context.Context) error { return runGatewayServers(ctx, m.runnables) }
	done := make(chan error, 1)
	go func() { done <- runGateway(t.Context(), options, deps) }()
	var addressError *net.OpError
	if err := awaitGatewayShutdown(t, done); !errors.As(err, &addressError) || addressError.Op != "listen" {
		t.Fatalf("startup = %v, want original listen error", err)
	}
	assertGatewaySetupCleanedUp(t, m)
}
