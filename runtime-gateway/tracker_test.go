package main

import (
	"context"
	"net/http"
	"testing"

	"k8s.io/apimachinery/pkg/util/sets"
)

// noopFallback records the host that triggered the fallback so tests can
// assert that resolution missed (or hit) the registered hostnames.
type noopFallback struct {
	invalidHost string
	noWorkHost  string
}

func (n *noopFallback) InvalidHostname(hostname string) (string, string) {
	n.invalidHost = hostname
	return "http", "fallback-invalid"
}

func (n *noopFallback) NoWorkloads(hostname string) (string, string) {
	n.noWorkHost = hostname
	return "http", "fallback-noworkloads"
}

func newTracker(t *testing.T) (*HostTracker, *noopFallback) {
	t.Helper()
	fb := &noopFallback{}
	ht := &HostTracker{
		Fallback:  fb,
		hosts:     make(map[string]string),
		hostnames: make(map[string]sets.Set[string]),
		workloads: make(map[string]string),
	}
	return ht, fb
}

// TestResolveStripsPortFromHostHeader covers the case where a browser sends
// "Host: localhost:8000" (non-standard port appended) but the workload was
// registered with the bare hostname "localhost". The CRD validates host
// values as RFC 1123 names so the registered value can never carry a port;
// the gateway must therefore match on the hostname portion of the header.
func TestResolveStripsPortFromHostHeader(t *testing.T) {
	ht, fb := newTracker(t)

	if err := ht.RegisterHost(context.Background(), "host-1", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost: %v", err)
	}
	if err := ht.RegisterWorkload(context.Background(), "host-1", "wl-1", "localhost"); err != nil {
		t.Fatalf("RegisterWorkload: %v", err)
	}

	req := &http.Request{Host: "localhost:8000"}
	res := ht.Resolve(context.Background(), req)

	if res.WorkloadID != "wl-1" {
		t.Fatalf("expected workload wl-1, got %q (fallback host=%q)", res.WorkloadID, fb.invalidHost)
	}
	if res.Hostname != "10.0.0.1:8080" {
		t.Fatalf("unexpected upstream hostname: %q", res.Hostname)
	}
}

// TestResolveExactMatchTakesPrecedence ensures we still honour an exact
// host:port registration (if one is ever introduced via a different code
// path) before falling back to the port-stripped lookup.
func TestResolveExactMatchTakesPrecedence(t *testing.T) {
	ht, _ := newTracker(t)

	if err := ht.RegisterHost(context.Background(), "host-a", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost a: %v", err)
	}
	if err := ht.RegisterHost(context.Background(), "host-b", "10.0.0.2", 8080); err != nil {
		t.Fatalf("RegisterHost b: %v", err)
	}
	if err := ht.RegisterWorkload(context.Background(), "host-a", "wl-bare", "example.com"); err != nil {
		t.Fatalf("RegisterWorkload bare: %v", err)
	}
	if err := ht.RegisterWorkload(context.Background(), "host-b", "wl-exact", "example.com:8443"); err != nil {
		t.Fatalf("RegisterWorkload exact: %v", err)
	}

	req := &http.Request{Host: "example.com:8443"}
	res := ht.Resolve(context.Background(), req)
	if res.WorkloadID != "wl-exact" {
		t.Fatalf("expected exact-match workload wl-exact, got %q", res.WorkloadID)
	}
}

// TestResolveUnknownHostFallsBack confirms that unknown hosts still hit the
// InvalidHostname fallback even after the port-stripping retry.
func TestResolveUnknownHostFallsBack(t *testing.T) {
	ht, fb := newTracker(t)

	if err := ht.RegisterHost(context.Background(), "host-1", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost: %v", err)
	}
	if err := ht.RegisterWorkload(context.Background(), "host-1", "wl-1", "localhost"); err != nil {
		t.Fatalf("RegisterWorkload: %v", err)
	}

	req := &http.Request{Host: "unknown.example:9000"}
	res := ht.Resolve(context.Background(), req)
	if res.WorkloadID != "" {
		t.Fatalf("expected fallback (no workload), got workload %q", res.WorkloadID)
	}
	if fb.invalidHost != "unknown.example:9000" {
		t.Fatalf("expected fallback to record original host header, got %q", fb.invalidHost)
	}
	if res.Hostname != "fallback-invalid" {
		t.Fatalf("expected fallback hostname, got %q", res.Hostname)
	}
}
