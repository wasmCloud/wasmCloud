package runtime

import (
	"context"
	"errors"
	"testing"

	"google.golang.org/protobuf/encoding/protojson"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// TestPlacementCarriesComponentInstanceLimits checks that every instance limit
// a component declares on the CRD reaches the host.
//
// Placement is the one place they are copied from the Kubernetes type to the
// proto one, field by field, and a limit that never arrives looks exactly like
// a limit that arrived and was honoured: the default for each is the
// conservative value, so an unset maxConcurrency and a dropped one both give
// one call at a time. The runtime pins the half below this (see
// `wire_limits_reach_the_runtime` in crates/wash-runtime); this pins the half
// above it, without needing a cluster.
func TestPlacementCarriesComponentInstanceLimits(t *testing.T) {
	reply, err := protojson.Marshal(&runtimev2.WorkloadStartResponse{
		WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: "workload-1"},
	})
	if err != nil {
		t.Fatalf("marshal start response: %v", err)
	}

	bus := &mockBus{reply: &wasmbus.Message{Data: reply}}
	r := &WorkloadReconciler{Bus: bus}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "limits", Namespace: metav1.NamespaceDefault},
		Spec: runtimev1alpha1.WorkloadSpec{
			Components: []runtimev1alpha1.WorkloadComponent{{
				Name:                 "pooled",
				Image:                "example.com/pooled:1",
				PoolSize:             4,
				MaxInvocations:       100,
				MaxConcurrency:       8,
				ReclaimWindowSeconds: 30,
				ReclaimMinInstances:  2,
			}},
		},
		Status: runtimev1alpha1.WorkloadStatus{HostID: "host-1"},
	}

	// Placement ends by skipping the rest of the reconciliation, having sent
	// the start request this test reads back.
	if err := r.reconcilePlacement(context.Background(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatalf("reconcilePlacement: %v", err)
	}

	var req runtimev2.WorkloadStartRequest
	if err := protojson.Unmarshal(bus.gotData, &req); err != nil {
		t.Fatalf("unmarshal start request: %v", err)
	}
	components := req.GetWorkload().GetWitWorld().GetComponents()
	if len(components) != 1 {
		t.Fatalf("got %d components, want 1", len(components))
	}

	got := components[0]
	for _, limit := range []struct {
		name string
		got  int32
		want int32
	}{
		{"poolSize", got.GetPoolSize(), 4},
		{"maxInvocations", got.GetMaxInvocations(), 100},
		{"maxConcurrency", got.GetMaxConcurrency(), 8},
		{"reclaimWindowSeconds", got.GetReclaimWindowSeconds(), 30},
		{"reclaimMinInstances", got.GetReclaimMinInstances(), 2},
	} {
		if limit.got != limit.want {
			t.Errorf("%s reached the host as %d, want %d", limit.name, limit.got, limit.want)
		}
	}
}

const testWorkloadUID = "workload-uid"

// A Workload deleted mid-start has a host but no recorded workload id; it must still be stopped.
func TestFinalizeStopsWorkloadWithoutRecordedPlacement(t *testing.T) {
	reply, err := protojson.Marshal(&runtimev2.WorkloadStopResponse{
		WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: testWorkloadUID},
	})
	if err != nil {
		t.Fatalf("marshal stop response: %v", err)
	}

	bus := &mockBus{reply: &wasmbus.Message{Data: reply}}
	r := &WorkloadReconciler{Bus: bus}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "deleted-mid-start", Namespace: "default", UID: testWorkloadUID},
		Status:     runtimev1alpha1.WorkloadStatus{HostID: "host-1"},
	}

	if err := r.finalize(context.Background(), workload); err != nil {
		t.Fatalf("finalize: %v", err)
	}

	if want := "runtime.host.host-1.workload.stop"; bus.gotSubject != want {
		t.Fatalf("stop sent to %q, want %q", bus.gotSubject, want)
	}
	var req runtimev2.WorkloadStopRequest
	if err := protojson.Unmarshal(bus.gotData, &req); err != nil {
		t.Fatalf("unmarshal stop request: %v", err)
	}
	if req.GetWorkloadId() != testWorkloadUID {
		t.Errorf("stop named workload %q, want the Workload's UID %q", req.GetWorkloadId(), testWorkloadUID)
	}
}

// No host was ever selected, so no start was sent and there is nothing to stop.
func TestFinalizeSkipsWorkloadWithoutHost(t *testing.T) {
	bus := &mockBus{err: errors.New("no request expected")}
	r := &WorkloadReconciler{Bus: bus}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "unscheduled", Namespace: "default", UID: testWorkloadUID},
	}

	if err := r.finalize(context.Background(), workload); err != nil {
		t.Fatalf("finalize: %v", err)
	}
	if bus.gotSubject != "" {
		t.Errorf("finalize sent %q for a Workload with no host", bus.gotSubject)
	}
}

// newWorkloadFinalizeClient builds a fake client wired with the HostID index
// finalize uses to decide whether a failed stop can be given up on.
func newWorkloadFinalizeClient(t *testing.T, objs ...client.Object) client.Client {
	t.Helper()
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	return fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.Host{}, hostIDIndex,
			func(obj client.Object) []string {
				host, ok := obj.(*runtimev1alpha1.Host)
				if !ok || host.HostID == "" {
					return nil
				}
				return []string{host.HostID}
			}).
		Build()
}

func placedWorkload() *runtimev1alpha1.Workload {
	w := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "placed"},
	}
	w.Status.HostID = testHeartbeatHostID
	w.Status.WorkloadID = "workload-id"
	w.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionPlacement))
	return w
}

// A stop that fails on a host still heartbeating may have left the workload
// running, so the finalizer must stay and the stop be retried.
func TestFinalizeRetriesFailedStopOnReadyHost(t *testing.T) {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "host", Namespace: testNamespace},
		HostID:     testHeartbeatHostID,
	}
	host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	stopErr := errors.New("nats: timeout")
	r := &WorkloadReconciler{
		Client:            newWorkloadFinalizeClient(t, host),
		Bus:               &mockBus{err: stopErr},
		OperatorNamespace: testNamespace,
	}

	if err := r.finalize(context.Background(), placedWorkload()); !errors.Is(err, stopErr) {
		t.Fatalf("finalize returned %v, want the stop error so it is retried", err)
	}
}

// A host that stopped heartbeating may never answer, and its Host may never be
// reaped, so waiting on it could block the deletion for good.
func TestFinalizeGivesUpFailedStopOnUnavailableHost(t *testing.T) {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "host", Namespace: testNamespace},
		HostID:     testHeartbeatHostID,
	}
	r := &WorkloadReconciler{
		Client:            newWorkloadFinalizeClient(t, host),
		Bus:               &mockBus{err: errors.New("nats: timeout")},
		OperatorNamespace: testNamespace,
	}

	if err := r.finalize(context.Background(), placedWorkload()); err != nil {
		t.Fatalf("finalize: %v", err)
	}
}

// Once the Host is gone there is nothing left to stop the workload on.
func TestFinalizeGivesUpFailedStopWhenHostIsGone(t *testing.T) {
	r := &WorkloadReconciler{
		Client:            newWorkloadFinalizeClient(t),
		Bus:               &mockBus{err: errors.New("nats: no responders available for request")},
		OperatorNamespace: testNamespace,
	}

	if err := r.finalize(context.Background(), placedWorkload()); err != nil {
		t.Fatalf("finalize: %v", err)
	}
}
