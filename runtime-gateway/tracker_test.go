package main

import (
	"net/http"
	"testing"

	"k8s.io/apimachinery/pkg/types"
)

const fallbackEndpoint = "127.0.0.1:1"

type testFallback struct{}

func (testFallback) InvalidHostname(string) (string, string) { return "http", fallbackEndpoint }
func (testFallback) NoWorkloads(string) (string, string)     { return "http", fallbackEndpoint }

func resolve(t *testing.T, ht *HostTracker, hostname string) LookupResult {
	t.Helper()
	return ht.Resolve(t.Context(), &http.Request{Host: hostname})
}

func TestHostTrackerRoutesRegisteredWorkload(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})

	hostKey := types.NamespacedName{Namespace: "ns", Name: "host-a"}
	workloadKey := types.NamespacedName{Namespace: "ns", Name: "workload-a"}

	if err := ht.RegisterHost(ctx, hostKey, "host-id", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost() = %v", err)
	}
	if err := ht.RegisterWorkload(ctx, workloadKey, "host-id", "workload-id", "a.example"); err != nil {
		t.Fatalf("RegisterWorkload() = %v", err)
	}

	got := resolve(t, ht, "a.example")
	if got.Hostname != "10.0.0.1:8080" || got.WorkloadID != "workload-id" {
		t.Errorf("Resolve() = %+v, want the registered host and workload", got)
	}
}

// Deregistration is keyed by the object key alone, because the reconciler that
// observes a deletion no longer has the object it was registered from.
func TestHostTrackerDeregisterWorkloadByKey(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})

	hostKey := types.NamespacedName{Namespace: "ns", Name: "host-a"}
	workloadKey := types.NamespacedName{Namespace: "ns", Name: "workload-a"}

	if err := ht.RegisterHost(ctx, hostKey, "host-id", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost() = %v", err)
	}
	if err := ht.RegisterWorkload(ctx, workloadKey, "host-id", "workload-id", "a.example"); err != nil {
		t.Fatalf("RegisterWorkload() = %v", err)
	}
	if err := ht.DeregisterWorkload(ctx, workloadKey); err != nil {
		t.Fatalf("DeregisterWorkload() = %v", err)
	}

	if got := resolve(t, ht, "a.example"); got.Hostname != fallbackEndpoint {
		t.Errorf("Resolve() = %+v, want the fallback endpoint", got)
	}
	// An unknown key is a no-op, not an error: a Workload the gateway never
	// routed still reaches the reconciler when it is deleted.
	if err := ht.DeregisterWorkload(ctx, types.NamespacedName{Namespace: "ns", Name: "unknown"}); err != nil {
		t.Errorf("DeregisterWorkload(unknown) = %v", err)
	}
}

func TestHostTrackerReregisterWorkloadDropsPreviousHostname(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})

	hostKey := types.NamespacedName{Namespace: "ns", Name: "host-a"}
	workloadKey := types.NamespacedName{Namespace: "ns", Name: "workload-a"}

	if err := ht.RegisterHost(ctx, hostKey, "host-id", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost() = %v", err)
	}
	if err := ht.RegisterWorkload(ctx, workloadKey, "host-id", "workload-id", "a.example"); err != nil {
		t.Fatalf("RegisterWorkload() = %v", err)
	}
	if err := ht.RegisterWorkload(ctx, workloadKey, "host-id", "workload-id", "b.example"); err != nil {
		t.Fatalf("RegisterWorkload() = %v", err)
	}

	if got := resolve(t, ht, "a.example"); got.Hostname != fallbackEndpoint {
		t.Errorf("Resolve(a.example) = %+v, want the fallback endpoint", got)
	}
	if got := resolve(t, ht, "b.example"); got.Hostname != "10.0.0.1:8080" {
		t.Errorf("Resolve(b.example) = %+v, want the registered host", got)
	}
}

func TestHostTrackerDeregisterHostDropsItsWorkloads(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})

	hostKey := types.NamespacedName{Namespace: "ns", Name: "host-a"}
	workloadKey := types.NamespacedName{Namespace: "ns", Name: "workload-a"}

	if err := ht.RegisterHost(ctx, hostKey, "host-id", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost() = %v", err)
	}
	if err := ht.RegisterWorkload(ctx, workloadKey, "host-id", "workload-id", "a.example"); err != nil {
		t.Fatalf("RegisterWorkload() = %v", err)
	}
	if err := ht.DeregisterHost(ctx, hostKey); err != nil {
		t.Fatalf("DeregisterHost() = %v", err)
	}

	if got := resolve(t, ht, "a.example"); got.Hostname != fallbackEndpoint {
		t.Errorf("Resolve() = %+v, want the fallback endpoint", got)
	}
	if len(ht.workloadKeys) != 0 {
		t.Errorf("workloadKeys = %v, want the host's workloads dropped", ht.workloadKeys)
	}
}

// A host pod that restarts re-registers under the same object name with a new
// host ID; the entry for the old ID must not keep taking traffic.
func TestHostTrackerReregisterHostReplacesPreviousID(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})

	hostKey := types.NamespacedName{Namespace: "ns", Name: "host-a"}

	if err := ht.RegisterHost(ctx, hostKey, "host-id-1", "10.0.0.1", 8080); err != nil {
		t.Fatalf("RegisterHost() = %v", err)
	}
	if err := ht.RegisterHost(ctx, hostKey, "host-id-2", "10.0.0.2", 8080); err != nil {
		t.Fatalf("RegisterHost() = %v", err)
	}

	if _, ok := ht.hosts["host-id-1"]; ok {
		t.Error("hosts still contains the replaced host ID")
	}
	if got := ht.hosts["host-id-2"]; got != "10.0.0.2:8080" {
		t.Errorf("hosts[host-id-2] = %q, want %q", got, "10.0.0.2:8080")
	}
}
