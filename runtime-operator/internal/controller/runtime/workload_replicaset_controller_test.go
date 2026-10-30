package runtime

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/labels"
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// The /scale subresource needs a flat current count and a selector that
// matches the Workloads the ReplicaSet creates.
func TestReplicaSet_ScaleSubresourceStatus(t *testing.T) {
	// A 63-character WorkloadDeployment name is valid, but the RS made from
	// it is longer than a label value may be.
	longRS := strings.Repeat("d", 63) + "-" + randHash()
	for _, name := range []string{testReplicaSetName, longRS} {
		t.Run(fmt.Sprintf("name length %d", len(name)), func(t *testing.T) {
			testReplicaSetScaleStatus(t, name)
		})
	}
}

func testReplicaSetScaleStatus(t *testing.T, replicaSetName string) {
	s := newScaleTestScheme(t)
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}

	replicas := int32(2)
	rs := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: replicaSetName, UID: "rs-uid"},
		Spec: runtimev1alpha1.WorkloadReplicaSetSpec{
			Replicas: &replicas,
			Template: runtimev1alpha1.WorkloadReplicaTemplate{
				Labels: map[string]string{"app": "echo-rs"},
			},
		},
	}
	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(rs).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetNameIndex, func(obj client.Object) []string {
			if name, ok := isOwnedByController(obj, replicaSetGvk); ok {
				return []string{name}
			}
			return nil
		}).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetHashIndex, func(obj client.Object) []string {
			return []string{obj.GetAnnotations()[workloadReplicaSetGenerationAnnotation]}
		}).
		Build()
	r := &WorkloadReplicaSetReconciler{Client: c, Scheme: s}
	ctx := context.Background()

	if err := r.reconcileScaleUp(ctx, rs); err != nil {
		t.Fatalf("reconcileScaleUp: %v", err)
	}
	rs.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp),
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleDown),
	)
	// The Workloads are not available yet, so Ready reports an error, but the
	// status fields are still filled in.
	_ = r.reconcileReady(ctx, rs)

	if rs.Status.CurrentReplicas != replicas {
		t.Fatalf("expected CurrentReplicas=%d, got %d", replicas, rs.Status.CurrentReplicas)
	}
	// labels.Parse rejects values the API server would.
	selector, err := labels.Parse(rs.Status.Selector)
	if err != nil {
		t.Fatalf("invalid Selector %q: %v", rs.Status.Selector, err)
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := c.List(ctx, workloads, client.MatchingLabelsSelector{Selector: selector}); err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(workloads.Items) != int(replicas) {
		t.Fatalf("expected the selector to match %d workloads, got %d", replicas, len(workloads.Items))
	}
	if workloads.Items[0].Labels["app"] != "echo-rs" {
		t.Fatalf("expected template labels kept, got %v", workloads.Items[0].Labels)
	}
	if rs.Spec.Template.Labels[workloadReplicaSetNameLabel] != "" {
		t.Fatalf("template labels were mutated: %v", rs.Spec.Template.Labels)
	}
}

// A refused Workload create is recorded on the ReplicaSet and on the
// WorkloadDeployment that owns it, not only in the ScaleUp condition.
func TestScaleUpRecordsRefusedWorkloadCreate(t *testing.T) {
	s := newScaleTestScheme(t)
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}
	rs := unavailableReplicaSet()
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
