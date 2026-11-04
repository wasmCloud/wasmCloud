package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"k8s.io/client-go/rest"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/healthz"
	"sigs.k8s.io/controller-runtime/pkg/manager"
)

type startupManager struct {
	ctrl.Manager
	client    client.Client
	calls     []string
	fail      string
	cause     error
	failed    bool
	runnables []manager.Runnable
	checks    map[string]healthz.Checker
	host      *HostReconciler
	workload  *WorkloadReconciler
	contexts  []context.Context
	start     func(context.Context) error
}

func (m *startupManager) step(name string) error {
	m.calls = append(m.calls, name)
	if name == m.fail && !m.failed {
		m.failed = true
		return m.cause
	}
	return nil
}

func (m *startupManager) GetClient() client.Client { return m.client }

func (m *startupManager) AddHealthzCheck(name string, check healthz.Checker) error {
	m.checks[name] = check
	return m.step(name)
}

func (m *startupManager) AddReadyzCheck(name string, check healthz.Checker) error {
	m.checks[name] = check
	return m.step(name)
}

func (m *startupManager) Add(r manager.Runnable) error {
	m.runnables = append(m.runnables, r)
	switch r.(type) {
	case *FallbackServer:
		return m.step("fallback")
	case *HTTPGateway:
		return m.step("gateway")
	default:
		return fmt.Errorf("unexpected runnable %T", r)
	}
}

func (m *startupManager) Start(ctx context.Context) error {
	m.contexts = append(m.contexts, ctx)
	if err := m.step("start"); err != nil {
		return err
	}
	if m.start != nil {
		return m.start(ctx)
	}
	return nil
}

func gatewayStartupFixture(t *testing.T) (gatewayOptions, gatewayDependencies, *startupManager) {
	t.Helper()
	options, err := parseGatewayOptions([]string{"--bind-addr=" + testGatewayBind, "--kubeconfig=test-config"}, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	m := &startupManager{
		client: fake.NewClientBuilder().WithScheme(scheme).Build(), checks: make(map[string]healthz.Checker),
		cause: errors.New("startup operation refused"),
	}
	config := &rest.Config{Host: testKubernetesHost}
	deps := defaultGatewayDependencies()
	deps.getConfig = func(path string) (*rest.Config, error) {
		if path != options.kubeconfig {
			t.Fatalf("kubeconfig path = %q", path)
		}
		return config, m.step("config")
	}
	deps.newManager = func(got *rest.Config, options ctrl.Options) (ctrl.Manager, error) {
		if got != config || options.Scheme != scheme {
			t.Fatal("manager lost its config or API scheme")
		}
		return m, m.step("manager")
	}
	deps.setupHost = func(r *HostReconciler, ctx context.Context, mgr ctrl.Manager) error {
		if mgr != m || r.Client != m.client {
			t.Fatal("host reconciler lost its manager or client")
		}
		m.host = r
		m.contexts = append(m.contexts, ctx)
		return m.step("host")
	}
	deps.setupWorkload = func(r *WorkloadReconciler, ctx context.Context, mgr ctrl.Manager) error {
		if mgr != m || r.Client != m.client {
			t.Fatal("workload reconciler lost its manager or client")
		}
		m.workload = r
		m.contexts = append(m.contexts, ctx)
		return m.step("workload")
	}
	return options, deps, m
}

func assertGatewaySetupCleanedUp(t *testing.T, m *startupManager) {
	t.Helper()
	for _, ctx := range m.contexts {
		if !errors.Is(ctx.Err(), context.Canceled) {
			t.Fatal("startup context was not canceled on return")
		}
	}
	for _, runnable := range m.runnables {
		if fallback, ok := runnable.(*FallbackServer); ok {
			assertGatewayListenerClosed(t, fallback.listener.Addr().String())
		}
	}
}

func TestGatewayStartupPropagatesErrorsAndReleasesResources(t *testing.T) {
	steps := []string{"config", "manager", "healthz", "readyz", "fallback", "gateway", "host", "workload", "start"}
	for i, step := range steps {
		t.Run(step, func(t *testing.T) {
			options, deps, m := gatewayStartupFixture(t)
			m.fail = step
			if err := runGateway(t.Context(), options, deps); !errors.Is(err, m.cause) {
				t.Fatalf("startup = %v, want original error", err)
			}
			if !reflect.DeepEqual(m.calls, steps[:i+1]) {
				t.Fatalf("startup calls = %v, want %v", m.calls, steps[:i+1])
			}
			assertGatewaySetupCleanedUp(t, m)
		})
	}
}

func TestGatewayStartupValidatesFallbackBeforeKubernetes(t *testing.T) {
	options, deps, m := gatewayStartupFixture(t)
	options.fallbackEndpoint = "ftp://fallback.example"
	if err := runGateway(t.Context(), options, deps); err == nil || !strings.Contains(err.Error(), "fallback endpoint") {
		t.Fatalf("startup = %v, want fallback validation error", err)
	}
	if len(m.calls) != 0 {
		t.Fatalf("invalid fallback caused startup calls: %v", m.calls)
	}
}

func TestGatewayStartupSharesRoutesAndConfiguresChecks(t *testing.T) {
	for _, endpoint := range []string{"", testFallbackOrigin} {
		t.Run(endpoint, func(t *testing.T) {
			options, deps, m := gatewayStartupFixture(t)
			options.fallbackEndpoint = endpoint
			type contextKey struct{}
			ctx := context.WithValue(t.Context(), contextKey{}, testGatewayMessage)
			m.start = func(ctx context.Context) error {
				if ctx.Value(contextKey{}) != testGatewayMessage {
					t.Fatal("manager lost context values")
				}
				var gateway *HTTPGateway
				var internal *FallbackServer
				for _, runnable := range m.runnables {
					switch r := runnable.(type) {
					case *HTTPGateway:
						gateway = r
					case *FallbackServer:
						internal = r
					}
				}
				if gateway == nil || gateway.BindAddr != options.bindAddr || gateway.Proxy.Rewrite == nil ||
					gateway.Proxy.ModifyResponse == nil || gateway.Proxy.ErrorHandler == nil {
					t.Fatalf("HTTP gateway wiring = %+v", gateway)
				}
				tracker, ok := gateway.Resolver.(*HostTracker)
				if !ok || m.host.Registry != tracker || m.workload.Registry != tracker {
					t.Fatal("gateway and reconcilers do not share the route registry")
				}
				transport, ok := gateway.Proxy.Transport.(*http.Transport)
				if !ok || transport.MaxIdleConns != 0 || transport.MaxIdleConnsPerHost != 100 ||
					transport.IdleConnTimeout != 90*time.Second {
					t.Fatalf("HTTP transport = %+v", gateway.Proxy.Transport)
				}
				lookup := tracker.Resolve(ctx, httptest.NewRequest(http.MethodGet, "http://unknown.example/path", nil))
				if endpoint == "" {
					if internal == nil || lookup.Scheme != "http" || lookup.Hostname != internal.listener.Addr().String() {
						t.Fatalf("internal fallback route = %+v", lookup)
					}
					host, _, err := net.SplitHostPort(lookup.Hostname)
					if err != nil || !net.ParseIP(host).IsLoopback() {
						t.Fatalf("internal fallback is not loopback: %q", lookup.Hostname)
					}
				} else if internal != nil || lookup.Scheme != "https" || lookup.Hostname != "fallback.example:8443" {
					t.Fatalf("external fallback route = %+v", lookup)
				}
				if len(m.checks) != 2 {
					t.Fatalf("health checks = %v", m.checks)
				}
				for _, name := range []string{"healthz", "readyz"} {
					if check := m.checks[name]; check == nil || check(httptest.NewRequest(http.MethodGet, "/", nil)) != nil {
						t.Fatalf("invalid %s check", name)
					}
				}
				return nil
			}
			if err := runGateway(ctx, options, deps); err != nil {
				t.Fatal(err)
			}
			assertGatewaySetupCleanedUp(t, m)
			if ctx.Err() != nil {
				t.Fatal("startup canceled the caller's context")
			}
		})
	}
}
