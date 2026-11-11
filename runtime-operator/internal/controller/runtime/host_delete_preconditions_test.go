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
	for _, path := range []string{"heartbeat", "pod"} {
		for _, recreated := range []bool{false, true} {
			name := path + "-updated"
			if recreated {
				name = path + "-recreated"
			}
			t.Run(name, func(t *testing.T) {
				testStaleHostDeletion(t, ctx, c, ns, name, path, recreated)
			})
		}
	}
}

func testStaleHostDeletion(t *testing.T, ctx context.Context, c client.Client, ns, name, path string, recreated bool) {
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
		host.Namespace, host.HostID = ns, "replacement-id"
		if err := c.Create(ctx, host); err != nil {
			t.Fatal(err)
		}
	} else {
		host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
		if err := c.Status().Update(ctx, host); err != nil {
			t.Fatal(err)
		}
	}

	var err error
	if path == "heartbeat" {
		err = (&HostReconciler{Client: c}).deleteUnresponsiveHost(ctx, snapshot)
	} else {
		retired := &RetiredHosts{}
		r := &HostPodReconciler{
			Client: &staleHostListClient{Client: c, snapshot: snapshot}, OperatorNamespace: ns, RetiredHosts: retired,
		}
		err = r.deleteHostForPod(ctx, terminatingPod(podIP, time.Now().Add(time.Minute), 0))
		if retired.contains(snapshot.HostID) || retired.fromRetiredPod(podIP, time.Time{}) {
			t.Error("a refused delete retired the Host or Pod")
		}
	}
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
