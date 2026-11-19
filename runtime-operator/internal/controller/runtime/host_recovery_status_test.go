package runtime

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
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

// A heartbeat that lands while a pass is deciding the Host is down refutes
// that verdict. The pass's conflicted status patch must not be reapplied with
// Ready false, or the post-hook deletes a Host that just reported; it is
// reapplied with Ready Unknown instead.
func TestHostHeartbeatMidPassSparesTheHost(t *testing.T) {
	c, mgr := newHostRecoveryManager(t)
	ctx := t.Context()
	ns := createTestNamespace(t, ctx, c, "host-heartbeat-mid-pass")
	const window = time.Minute
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "late-heartbeat", Namespace: ns, Finalizers: []string{hostFinalizerName}},
		HostID:     "late-heartbeat-id",
	}
	if err := c.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	// Last seen past the window: a pass whose probe fails concludes Ready false
	// unless something changes.
	host.Status.LastSeen = metav1.NewTime(time.Now().Add(-2 * window))
	if err := c.Status().Update(ctx, host); err != nil {
		t.Fatal(err)
	}

	// The probe times out. On the second pass, which reaches the Ready verdict,
	// the host's own heartbeat lands while the probe is waiting.
	landHeartbeat := false
	bus := &recoveryHeartbeatBus{heartbeat: func(ctx context.Context, _ *wasmbus.Message) (*wasmbus.Message, error) {
		if !landHeartbeat {
			return nil, errors.New("probe timed out")
		}
		patch, err := hostStatusPatch("")
		if err != nil {
			return nil, err
		}
		if err := c.Status().Patch(ctx, host.DeepCopy(), client.RawPatch(types.MergePatchType, patch)); err != nil {
			return nil, err
		}
		return nil, errors.New("probe timed out")
	}}
	r := &HostReconciler{Client: c, Scheme: c.Scheme(), Bus: bus, UnreachableTimeout: window}
	r.fleet.heard(time.Now().Add(-2 * window))
	r.fleet.observe(time.Now().Add(-2*window), window)
	r.fleet.heard(time.Now())
	if err := r.SetupWithManager(mgr); err != nil {
		t.Fatal(err)
	}

	// The first pass records Reporting false and stops there.
	req := ctrl.Request{NamespacedName: client.ObjectKeyFromObject(host)}
	if _, err := r.Reconcile(ctx, req); err != nil {
		t.Fatal(err)
	}
	landHeartbeat = true
	result, err := r.Reconcile(ctx, req)
	if err != nil || result.RequeueAfter <= 0 {
		t.Fatalf("pass should be written and requeued: result=%+v, err=%v", result, err)
	}
	current := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKeyFromObject(host), current); err != nil {
		t.Fatal(err)
	}
	if !current.DeletionTimestamp.IsZero() {
		t.Fatal("a Host that heartbeated mid-pass was deleted")
	}
	// The pass is still written, with Ready downgraded to what the new
	// LastSeen supports, so a host whose probes fail while it heartbeats keeps
	// its status moving.
	ready := current.Status.GetCondition(condition.TypeReady)
	if ready.Status != condition.ConditionUnknown || !strings.Contains(ready.Message, errHostNotReporting.Error()) {
		t.Fatalf("Ready = %s %q, want Unknown: %v", ready.Status, ready.Message, errHostNotReporting)
	}
	t.Cleanup(func() {
		current.Finalizers = nil
		_ = c.Update(context.Background(), current)
	})
}
