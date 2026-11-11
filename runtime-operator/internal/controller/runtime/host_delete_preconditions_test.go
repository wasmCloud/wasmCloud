package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
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

	err := (&HostReconciler{Client: c}).deleteUnresponsiveHost(ctx, snapshot)
	if !apierrors.IsConflict(err) {
		t.Fatalf("stale deletion should conflict, got %v", err)
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
