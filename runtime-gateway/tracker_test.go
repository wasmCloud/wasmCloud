package main

import (
	"fmt"
	"maps"
	"math/rand/v2"
	"net/http"
	"testing"

	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/sets"
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

func TestHostTrackerWorkloadBeforeHost(t *testing.T) {
	tracker := newHostTracker(&ExternalFallback{Scheme: "https", Endpoint: "fallback.example"})
	key := types.NamespacedName{Namespace: "ns", Name: "workload"}
	if err := tracker.RegisterWorkload(t.Context(), key, "host-id", "workload-id", "route.example"); err != nil {
		t.Fatal(err)
	}
	if got := resolve(t, tracker, "route.example"); got != (LookupResult{Scheme: "https", Hostname: "fallback.example"}) {
		t.Fatalf("host not registered yet: %+v", got)
	}
	if err := tracker.RegisterHost(t.Context(), key, "host-id", "::1", 8080); err != nil {
		t.Fatal(err)
	}
	if got := resolve(t, tracker, "route.example"); got.Hostname != "[::1]:8080" || got.WorkloadID != "workload-id" {
		t.Fatalf("host registered: %+v", got)
	}
}

func TestHostTrackerRemovalPreservesOtherRoutes(t *testing.T) {
	tracker := newHostTracker(testFallback{})
	for _, id := range []string{"a", "b"} {
		key := types.NamespacedName{Namespace: "ns", Name: id}
		if err := tracker.RegisterHost(t.Context(), key, id, id, 8080); err != nil {
			t.Fatal(err)
		}
		if err := tracker.RegisterWorkload(t.Context(), key, id, id, "shared.example"); err != nil {
			t.Fatal(err)
		}
	}
	key := types.NamespacedName{Namespace: "ns", Name: "a"}
	for range 2 {
		if err := tracker.DeregisterHost(t.Context(), key); err != nil {
			t.Fatal(err)
		}
		if got := resolve(t, tracker, "shared.example"); got.Hostname != "b:8080" || got.WorkloadID != "b" {
			t.Fatalf("remaining route: %+v", got)
		}
	}
	if err := tracker.DeregisterWorkload(t.Context(), types.NamespacedName{Namespace: "ns", Name: "b"}); err != nil {
		t.Fatal(err)
	}
	if got := resolve(t, tracker, "shared.example"); got.Hostname != fallbackEndpoint {
		t.Fatalf("removed last workload: %+v", got)
	}
}

func TestHostTrackerWorkloadMovePreservesSharedRoute(t *testing.T) {
	tracker := newHostTracker(testFallback{})
	first := types.NamespacedName{Namespace: "ns", Name: "first"}
	second := types.NamespacedName{Namespace: "ns", Name: "second"}
	for _, key := range []types.NamespacedName{first, second} {
		if err := tracker.RegisterHost(t.Context(), key, key.Name, key.Name, 8080); err != nil {
			t.Fatal(err)
		}
		if err := tracker.RegisterWorkload(t.Context(), key, key.Name, key.Name, "shared.example"); err != nil {
			t.Fatal(err)
		}
	}
	for range 2 {
		if err := tracker.RegisterWorkload(t.Context(), first, second.Name, first.Name, "shared.example"); err != nil {
			t.Fatal(err)
		}
	}
	if err := tracker.DeregisterHost(t.Context(), first); err != nil {
		t.Fatal(err)
	}
	if got := tracker.workloads[first.Name]; got != second.Name {
		t.Fatalf("moved workload host = %q, want %q", got, second.Name)
	}
	if got := tracker.hostnames["shared.example"].Len(); got != 2 {
		t.Fatalf("shared route has %d workloads after move, want two", got)
	}
	if err := tracker.DeregisterWorkload(t.Context(), second); err != nil {
		t.Fatal(err)
	}
	want := LookupResult{Scheme: "http", Hostname: "second:8080", WorkloadID: first.Name}
	if got := resolve(t, tracker, "shared.example"); got != want {
		t.Fatalf("route after removing the other workload = %+v, want %+v", got, want)
	}
	if err := tracker.RegisterHost(t.Context(), second, second.Name, "updated", 9090); err != nil {
		t.Fatal(err)
	}
	want.Hostname = "updated:9090"
	if got := resolve(t, tracker, "shared.example"); got != want {
		t.Fatalf("route after host address update = %+v, want %+v", got, want)
	}
}

// A suspended host that is then deleted loses its workload routes, as one
// deleted while Ready does.
func TestHostTrackerDeregisterSuspendedHostDropsItsWorkloads(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})
	hostKey := types.NamespacedName{Namespace: "ns", Name: "host-a"}
	workloadKey := types.NamespacedName{Namespace: "ns", Name: "workload-a"}
	if err := ht.RegisterHost(ctx, hostKey, "host-id", "10.0.0.1", 8080); err != nil {
		t.Fatal(err)
	}
	if err := ht.RegisterWorkload(ctx, workloadKey, "host-id", "workload-id", "a.example"); err != nil {
		t.Fatal(err)
	}
	if err := ht.SuspendHost(ctx, hostKey, "host-id"); err != nil {
		t.Fatal(err)
	}
	if err := ht.DeregisterHost(ctx, hostKey); err != nil {
		t.Fatal(err)
	}
	if len(ht.workloadKeys) != 0 {
		t.Errorf("workloadKeys = %v, want the deleted host's workloads dropped", ht.workloadKeys)
	}
	if err := ht.RegisterHost(ctx, types.NamespacedName{Namespace: "ns", Name: "host-b"}, "host-id", "10.0.0.2", 8080); err != nil {
		t.Fatal(err)
	}
	if got := resolve(t, ht, "a.example"); got.Hostname != fallbackEndpoint {
		t.Errorf("Resolve() = %+v after the host was deleted, want the fallback", got)
	}
}

// A Host suspended under a new ID is a restarted host: the previous ID takes
// no more traffic and its workload routes go, as RegisterHost does.
func TestHostTrackerSuspendUnderNewIDDropsPreviousHost(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})
	hostKey := types.NamespacedName{Namespace: "ns", Name: "host-a"}
	workloadKey := types.NamespacedName{Namespace: "ns", Name: "workload-a"}
	if err := ht.RegisterHost(ctx, hostKey, "old-id", "10.0.0.1", 8080); err != nil {
		t.Fatal(err)
	}
	if err := ht.RegisterWorkload(ctx, workloadKey, "old-id", "workload-id", "a.example"); err != nil {
		t.Fatal(err)
	}
	if err := ht.SuspendHost(ctx, hostKey, "new-id"); err != nil {
		t.Fatal(err)
	}
	if got := resolve(t, ht, "a.example"); got.Hostname != fallbackEndpoint {
		t.Errorf("Resolve() = %+v after the host restarted, want the fallback", got)
	}
	if len(ht.workloadKeys) != 0 {
		t.Errorf("workloadKeys = %v, want the previous ID's workloads dropped", ht.workloadKeys)
	}
}

// Resolve spreads a hostname's requests evenly across the replicas whose hosts
// take traffic, while some hosts are suspended and while none are.
func TestHostTrackerSpreadsRequestsAcrossReadyReplicas(t *testing.T) {
	for name, suspended := range map[string][]string{
		"all ready":      nil,
		"some suspended": {"r0", "r2"},
	} {
		t.Run(name, func(t *testing.T) {
			ctx := t.Context()
			ht := newHostTracker(testFallback{})
			for _, id := range []string{"r0", "r1", "r2", "r3"} {
				key := types.NamespacedName{Namespace: "ns", Name: id}
				if err := ht.RegisterHost(ctx, key, id, id, 8080); err != nil {
					t.Fatal(err)
				}
				if err := ht.RegisterWorkload(ctx, key, id, id, "shared.example"); err != nil {
					t.Fatal(err)
				}
			}
			for _, id := range suspended {
				if err := ht.SuspendHost(ctx, types.NamespacedName{Namespace: "ns", Name: id}, id); err != nil {
					t.Fatal(err)
				}
			}
			const requests = 20000
			picks := map[string]int{}
			for range requests {
				picks[resolve(t, ht, "shared.example").WorkloadID]++
			}
			ready := 4 - len(suspended)
			if len(picks) != ready {
				t.Fatalf("picks = %v, want %d ready replicas", picks, ready)
			}
			// At 20000 requests a fair share varies by about 1% of its mean; 20% is far outside chance.
			for id, n := range picks {
				if share, want := float64(n)/requests, 1/float64(ready); share < want*0.8 || share > want*1.2 {
					t.Errorf("replica %s took %.3f of the requests, want about %.3f: %v", id, share, want, picks)
				}
			}
		})
	}
}

// Random Host and Workload changes keep every hostname's ready workloads equal
// to those whose host takes traffic.
func TestHostTrackerReadyFollowsRegistrations(t *testing.T) {
	ctx := t.Context()
	ht := newHostTracker(testFallback{})
	r := rand.New(rand.NewPCG(1, 2))
	hostKey := func() types.NamespacedName {
		return types.NamespacedName{Namespace: "ns", Name: fmt.Sprintf("host-%d", r.IntN(3))}
	}
	// A key's ID changes when its host restarts; IDs are never shared between keys.
	hostID := func(key types.NamespacedName) string { return fmt.Sprintf("%s-%d", key.Name, r.IntN(2)) }
	for step := range 5000 {
		var op string
		switch r.IntN(5) {
		case 0:
			key := hostKey()
			id := hostID(key)
			op = "RegisterHost " + id
			if err := ht.RegisterHost(ctx, key, id, id, 8080); err != nil {
				t.Fatal(err)
			}
		case 1:
			key := hostKey()
			id := hostID(key)
			op = "SuspendHost " + id
			if err := ht.SuspendHost(ctx, key, id); err != nil {
				t.Fatal(err)
			}
		case 2:
			key := hostKey()
			op = "DeregisterHost " + key.Name
			if err := ht.DeregisterHost(ctx, key); err != nil {
				t.Fatal(err)
			}
		case 3:
			name := fmt.Sprintf("workload-%d", r.IntN(6))
			id, hostname := hostID(hostKey()), fmt.Sprintf("%d.example", r.IntN(2))
			op = fmt.Sprintf("RegisterWorkload %s on %s for %s", name, id, hostname)
			if err := ht.RegisterWorkload(ctx, types.NamespacedName{Namespace: "ns", Name: name}, id, name, hostname); err != nil {
				t.Fatal(err)
			}
		case 4:
			name := fmt.Sprintf("workload-%d", r.IntN(6))
			op = "DeregisterWorkload " + name
			if err := ht.DeregisterWorkload(ctx, types.NamespacedName{Namespace: "ns", Name: name}); err != nil {
				t.Fatal(err)
			}
		}
		want := map[string]sets.Set[string]{}
		for hostname, workloadSet := range ht.hostnames {
			for workloadID := range workloadSet {
				if _, ok := ht.hosts[ht.workloads[workloadID]]; ok {
					if want[hostname] == nil {
						want[hostname] = sets.New[string]()
					}
					want[hostname].Insert(workloadID)
				}
			}
		}
		got := map[string]sets.Set[string]{}
		for hostname, ready := range ht.ready {
			got[hostname] = sets.New(ready...)
			if got[hostname].Len() != len(ready) {
				t.Fatalf("step %d, %s: ready[%s] = %v has duplicates", step, op, hostname, ready)
			}
		}
		if !maps.EqualFunc(got, want, sets.Set[string].Equal) {
			t.Fatalf("step %d, %s: ready = %v, want %v", step, op, got, want)
		}
	}
}
