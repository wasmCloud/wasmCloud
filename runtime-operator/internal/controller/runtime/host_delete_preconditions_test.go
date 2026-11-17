package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	replacementHostID = "replacement-id"
	hostUpdated       = "updated"
	hostRecreated     = "recreated"
	hostMoved         = "moved"
)

// Model an indexed cache read that still contains the previous Host snapshot.
// Deletes go to the real API server so both UID and version preconditions apply.
type staleHostListClient struct {
	client.Client
	snapshot *runtimev1alpha1.Host
}

func (c *staleHostListClient) List(ctx context.Context, list client.ObjectList, opts ...client.ListOption) error {
	if hosts, ok := list.(*runtimev1alpha1.HostList); ok {
		hosts.Items = []runtimev1alpha1.Host{*c.snapshot.DeepCopy()}
		return nil
	}
	return c.Client.List(ctx, list, opts...)
}

// A stale snapshot never deletes a Host that recovered or was replaced, and the
// conflict that reveals the change is not a reconcile failure: the Host is
// reread and the decision made again.
func TestHostDeletionRejectsStaleSnapshots(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-delete-preconditions")
	for _, recreated := range []bool{false, true} {
		name := "heartbeat-updated"
		if recreated {
			name = "heartbeat-recreated"
		}
		t.Run(name, func(t *testing.T) {
			testStaleHostDeletion(t, ctx, c, ns, name, recreated)
		})
	}
}

// Only the resource version moved (for example this pass's own status patch
// not yet in the cache): the Host is still down, so it is deleted.
func TestHostDeletionRereadsAStillDownHost(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-delete-reread")
	host := hostAt("still-down", "10.1.2.6", time.Now())
	host.Namespace = ns
	if err := c.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	host.Status.SetConditions(condition.ErrorCondition(condition.TypeReady, condition.ReasonUnavailable, errors.New("host down")))
	if err := c.Status().Update(ctx, host); err != nil {
		t.Fatal(err)
	}
	snapshot := host.DeepCopy()
	host.Status.Version = "moved-on"
	if err := c.Status().Update(ctx, host); err != nil {
		t.Fatal(err)
	}
	if err := (&HostReconciler{Client: c, apiReader: c}).deleteUnresponsiveHost(ctx, snapshot); err != nil {
		t.Fatalf("a conflict on a still-down Host failed the pass: %v", err)
	}
	if err := c.Get(ctx, client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
		t.Fatalf("still-down Host was not deleted: %v", err)
	}
}

func testStaleHostDeletion(t *testing.T, ctx context.Context, c client.Client, ns, name string, recreated bool) {
	t.Helper()
	const podIP = "10.1.2.3"
	host := hostAt(name, podIP, time.Now())
	host.Namespace = ns
	if err := c.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	host.Status.SetConditions(condition.ErrorCondition(condition.TypeReady, condition.ReasonUnavailable, errors.New("host down")))
	if err := c.Status().Update(ctx, host); err != nil {
		t.Fatal(err)
	}
	snapshot := host.DeepCopy()
	if recreated {
		if err := c.Delete(ctx, host); err != nil {
			t.Fatal(err)
		}
		host = hostAt(name, podIP, time.Now())
		host.Namespace, host.HostID = ns, replacementHostID
		if err := c.Create(ctx, host); err != nil {
			t.Fatal(err)
		}
	} else {
		host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
		if err := c.Status().Update(ctx, host); err != nil {
			t.Fatal(err)
		}
	}

	if err := (&HostReconciler{Client: c, apiReader: c}).deleteUnresponsiveHost(ctx, snapshot); err != nil {
		t.Fatalf("stale deletion should be spared quietly, got %v", err)
	}
	current := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKeyFromObject(host), current); err != nil {
		t.Fatal(err)
	}
	if current.UID != host.UID || !current.DeletionTimestamp.IsZero() {
		t.Fatal("stale deletion removed the current Host")
	}
}

// A draining host keeps heartbeating, so the Pod path rereads on conflict. It
// still deletes the same Host, but spares a replacement or a Host now at a new IP.
func TestPodHostDeletionRereadsStaleSnapshots(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "pod-host-delete-reread")
	for _, change := range []string{hostUpdated, hostRecreated, hostMoved} {
		t.Run(change, func(t *testing.T) {
			testPodStaleHostDeletion(t, ctx, c, ns, "pod-"+change, change)
		})
	}
}

func testPodStaleHostDeletion(t *testing.T, ctx context.Context, c client.Client, ns, name, change string) {
	t.Helper()
	const podIP = "10.1.2.4"
	host := hostAt(name, podIP, time.Now())
	host.Namespace = ns
	if err := c.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	snapshot := host.DeepCopy()
	switch change {
	case hostUpdated:
		host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
		if err := c.Status().Update(ctx, host); err != nil {
			t.Fatal(err)
		}
	case hostRecreated:
		if err := c.Delete(ctx, host); err != nil {
			t.Fatal(err)
		}
		host = hostAt(name, podIP, time.Now())
		host.Namespace, host.HostID = ns, replacementHostID
		if err := c.Create(ctx, host); err != nil {
			t.Fatal(err)
		}
	case hostMoved:
		host.Hostname = "10.1.2.5"
		if err := c.Update(ctx, host); err != nil {
			t.Fatal(err)
		}
	}

	retired := &RetiredHosts{}
	r := &HostPodReconciler{
		Client: &staleHostListClient{Client: c, snapshot: snapshot}, OperatorNamespace: ns, RetiredHosts: retired,
	}
	if err := r.deleteHostForPod(ctx, terminatingPod(podIP, time.Now().Add(time.Minute), 0)); err != nil {
		t.Fatalf("deleteHostForPod: %v", err)
	}

	current := &runtimev1alpha1.Host{}
	err := c.Get(ctx, client.ObjectKeyFromObject(host), current)
	if change == hostUpdated {
		if !apierrors.IsNotFound(err) && (err != nil || current.DeletionTimestamp.IsZero()) {
			t.Fatalf("updated Host was not deleted: %v", err)
		}
		if !retired.contains(snapshot.HostID) {
			t.Error("deleted Host was not retired")
		}
		return
	}
	if err != nil {
		t.Fatal(err)
	}
	if current.UID != host.UID || !current.DeletionTimestamp.IsZero() {
		t.Fatal("stale deletion removed the current Host")
	}
	if retired.contains(host.HostID) {
		t.Error("the current Host was retired")
	}
	if change == hostMoved && retired.contains(snapshot.HostID) {
		t.Error("a Host that moved to another IP was retired")
	}
}

// countingDeleteClient counts Host deletes and fails the first with a
// conflict, as a draining host's heartbeat would.
type countingDeleteClient struct {
	*staleHostListClient
	deletes int
}

func (c *countingDeleteClient) Delete(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
	c.deletes++
	return c.staleHostListClient.Delete(ctx, obj, opts...)
}

// A Host the Pod path finds already terminating is gone as far as the Pod is
// concerned: its own finalizer finishes, and deleting it again would race the
// draining host's heartbeats until the retries ran out.
func TestPodHostDeletionTreatsTerminatingHostAsGone(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "pod-host-terminating")
	const podIP = "10.1.2.7"
	host := hostAt("pod-terminating", podIP, time.Now())
	host.Namespace = ns
	host.Finalizers = []string{"test.wasmcloud.dev/hold"}
	if err := c.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	snapshot := host.DeepCopy()
	if err := c.Delete(ctx, host); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		current := &runtimev1alpha1.Host{}
		if err := c.Get(ctx, client.ObjectKeyFromObject(host), current); err == nil {
			current.Finalizers = nil
			_ = c.Update(ctx, current)
		}
	})

	retired := &RetiredHosts{}
	counting := &countingDeleteClient{staleHostListClient: &staleHostListClient{Client: c, snapshot: snapshot}}
	r := &HostPodReconciler{Client: counting, OperatorNamespace: ns, RetiredHosts: retired}
	if err := r.deleteHostForPod(ctx, terminatingPod(podIP, time.Now().Add(time.Minute), 0)); err != nil {
		t.Fatalf("deleteHostForPod: %v", err)
	}
	// The stale snapshot's delete conflicts; the reread sees the deletion and
	// stops there.
	if counting.deletes != 1 {
		t.Fatalf("terminating Host deleted %d times, want 1", counting.deletes)
	}
	if !retired.contains(snapshot.HostID) {
		t.Error("terminating Host was not retired")
	}
}

func TestHostStatusRebaseKeepsNewerHeartbeat(t *testing.T) {
	earlier, later := metav1.NewTime(time.Now().Add(-time.Second)), metav1.Now()
	for _, tt := range []struct {
		name         string
		fresh, probe metav1.Time
		want         metav1.Time
	}{
		{name: "heartbeat landed after the probe", fresh: later, probe: earlier, want: later},
		{name: "probe is newer", fresh: earlier, probe: later, want: later},
	} {
		t.Run(tt.name, func(t *testing.T) {
			fresh, desired := &runtimev1alpha1.Host{}, &runtimev1alpha1.Host{}
			fresh.Status.LastSeen, desired.Status.LastSeen = tt.fresh, tt.probe
			keepNewerLastSeen(fresh, desired)
			if !desired.Status.LastSeen.Equal(&tt.want) {
				t.Fatalf("LastSeen = %v, want %v", desired.Status.LastSeen, tt.want)
			}
		})
	}
}
