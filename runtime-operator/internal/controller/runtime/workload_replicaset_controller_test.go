package runtime

import (
	"context"
	"errors"
	"strings"
	"testing"

	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// A refused Workload create is recorded on the ReplicaSet and on the
// WorkloadDeployment that owns it, not only in the ScaleUp condition.
func TestScaleUpRecordsRefusedWorkloadCreate(t *testing.T) {
	s := newScaleTestScheme(t)
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}
	rs := unavailableReplicaSet(testReplicaSetName)
	recorder := events.NewFakeRecorder(4)
	r := &WorkloadReplicaSetReconciler{
		Client: fake.NewClientBuilder().
			WithScheme(s).
			WithObjects(rs).
			WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetNameIndex, replicaSetNameIndexValue(replicaSetGvk)).
			WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetHashIndex, replicaSetHashIndexValue(replicaSetGvk)).
			WithInterceptorFuncs(refuseCreates).
			Build(),
		Scheme:   s,
		Recorder: recorder,
	}

	err = r.reconcileScaleUp(context.Background(), rs)
	if err == nil || !strings.Contains(err.Error(), "require-team-label") {
		t.Fatalf("scale up error = %v, want the refusal", err)
	}
	got := drainEvents(recorder)
	if len(got) != 2 {
		t.Fatalf("events = %q, want one on the ReplicaSet and one on the deployment", got)
	}
	for _, event := range got {
		if !strings.HasPrefix(event, "Warning FailedCreate") || !strings.Contains(event, "require-team-label") {
			t.Errorf("event %q does not carry the refusal", event)
		}
	}
	if !strings.Contains(got[1], testReplicaSetName) {
		t.Errorf("deployment event %q does not name the ReplicaSet", got[1])
	}

	// The generated Workload name is left out, so a repeated refusal reads
	// the same and its condition is not rewritten on every pass.
	again := r.reconcileScaleUp(context.Background(), rs)
	if again == nil || again.Error() != err.Error() || strings.Contains(err.Error(), "\"") {
		t.Errorf("refusals read %q then %q, want one message without the generated name", err, again)
	}
}

// A ScaleUp failure is why the ReplicaSet is not ready, so Ready says so.
func TestReplicaSetReadyCarriesScaleUpFailure(t *testing.T) {
	rs := baseReplicaSet(testReplicaSetName)
	rs.Status.SetConditions(condition.ErrorCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp,
		"Reconcile", errors.New("creating Workload: denied by policy require-team-label")))
	r := &WorkloadReplicaSetReconciler{}

	err := r.reconcileReady(context.Background(), rs)
	if err == nil || condition.IsStatusUnknown(err) || !strings.Contains(err.Error(), "require-team-label") {
		t.Fatalf("ready = %v, want a failure carrying the ScaleUp message", err)
	}
}
