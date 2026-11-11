package runtime

import (
	"context"
	"testing"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

func TestHostProbeCannotOverwriteReplacementStatus(t *testing.T) {
	c, mgr := newHostRecoveryManager(t)
	ctx := t.Context()
	ns := createTestNamespace(t, ctx, c, "host-probe-replacement")
	old := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "reused-name", Namespace: ns, Finalizers: []string{hostFinalizerName}},
		HostID:     "departing-id",
	}
	if err := c.Create(ctx, old); err != nil {
		t.Fatal(err)
	}
	replacement := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: old.Name, Namespace: ns}, HostID: "new-id",
	}
	// Simulate cleanup completing and a replacement registering between the
	// reporting pass's read and its heartbeat response. Use the real API server
	// to enforce the subsequent status patch's resource-version precondition.
	bus := &recoveryHeartbeatBus{heartbeat: func(ctx context.Context, _ *wasmbus.Message) (*wasmbus.Message, error) {
		if err := c.Delete(ctx, old); err != nil {
			return nil, err
		}
		if err := c.Get(ctx, client.ObjectKeyFromObject(old), old); err != nil {
			return nil, err
		}
		old.Finalizers = nil
		if err := c.Update(ctx, old); err != nil {
			return nil, err
		}
		if err := c.Create(ctx, replacement); err != nil {
			return nil, err
		}
		replacement.Status.OSName = "replacement-os"
		if err := c.Status().Update(ctx, replacement); err != nil {
			return nil, err
		}
		return &wasmbus.Message{Data: []byte(`{"osName":"departing-os"}`)}, nil
	}}
	r := &HostReconciler{Client: c, Scheme: c.Scheme(), Bus: bus, UnreachableTimeout: time.Minute}
	if err := r.SetupWithManager(mgr); err != nil {
		t.Fatal(err)
	}
	result, err := r.Reconcile(ctx, ctrl.Request{NamespacedName: client.ObjectKeyFromObject(old)})
	if err != nil || result.RequeueAfter <= 0 {
		t.Fatalf("stale status should requeue: result=%+v, err=%v", result, err)
	}
	current := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKeyFromObject(replacement), current); err != nil {
		t.Fatal(err)
	}
	if current.UID != replacement.UID || current.Status.OSName != replacement.Status.OSName || !current.DeletionTimestamp.IsZero() {
		t.Fatalf("old probe changed its replacement: uid=%s, os=%s, deleting=%v", current.UID, current.Status.OSName, current.DeletionTimestamp)
	}
}
