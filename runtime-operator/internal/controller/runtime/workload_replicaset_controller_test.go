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
const testReplicaSetUID = "rs-uid"

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
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: replicaSetName, UID: testReplicaSetUID},
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
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, func(obj client.Object) []string {
			if ownerUID, ok := controllerUID(obj, replicaSetGvk); ok {
				return []string{ownerUID}
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

// Deleting and re-applying a ReplicaSet reuses its name. The old Workloads are
// on their way out, so the new one starts its own and doesn't report them.
func TestReplicaSet_PredecessorWorkloadsDoNotCount(t *testing.T) {
	s := newScaleTestScheme(t)
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}

	replicas := int32(1)
	rs := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: testReplicaSetName, UID: testReplicaSetUID},
		Spec:       runtimev1alpha1.WorkloadReplicaSetSpec{Replicas: &replicas},
	}
	controller := true
	old := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{
			Namespace:   testNamespace,
			Name:        testReplicaSetName + "-old",
			Annotations: map[string]string{workloadReplicaSetGenerationAnnotation: rs.Spec.Template.Hash()},
			Labels: map[string]string{
				workloadReplicaSetNameLabel: ownerLabelValue(testReplicaSetName),
				workloadReplicaSetUIDLabel:  "deleted-rs-uid",
			},
			OwnerReferences: []metav1.OwnerReference{{
				APIVersion: runtimev1alpha1.GroupVersion.String(),
				Kind:       replicaSetGvk.Kind,
				Name:       testReplicaSetName,
				UID:        "deleted-rs-uid",
				Controller: &controller,
			}},
		},
	}
	old.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(rs, old).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, func(obj client.Object) []string {
			if ownerUID, ok := controllerUID(obj, replicaSetGvk); ok {
				return []string{ownerUID}
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
	if err := r.reconcileReady(ctx, rs); err == nil {
		t.Fatal("expected Ready to wait for the new Workload")
	}
	if rs.Status.Replicas.Current != 1 || rs.Status.Replicas.Ready != 0 {
		t.Fatalf("expected only the new Workload counted, got %+v", rs.Status.Replicas)
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := c.List(ctx, workloads); err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(workloads.Items) != 2 {
		t.Fatalf("expected the old Workload left for the garbage collector and one new, got %d", len(workloads.Items))
	}

	// What an autoscaler reads through /scale has to agree with currentReplicas.
	selector, err := labels.Parse(rs.Status.Selector)
	if err != nil {
		t.Fatalf("invalid Selector %q: %v", rs.Status.Selector, err)
	}
	if err := c.List(ctx, workloads, client.MatchingLabelsSelector{Selector: selector}); err != nil {
		t.Fatalf("list by selector: %v", err)
	}
	if len(workloads.Items) != 1 || workloads.Items[0].Name == old.Name {
		t.Fatalf("expected the selector to match only the new Workload, got %d", len(workloads.Items))
	}
}

// A ReplicaSet being deleted starts and removes nothing: its Workloads are the
// garbage collector's, and --cascade=orphan means to keep them. Its status
// still follows what is left, and it stops being Ready as the delete starts.
func TestReplicaSet_DeleteChangesNothing(t *testing.T) {
	s := newScaleTestScheme(t)
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}

	replicas, none := int32(2), int32(0)
	now := metav1.Now()
	rs := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{
			Namespace:         testNamespace,
			Name:              testReplicaSetName,
			UID:               testReplicaSetUID,
			DeletionTimestamp: &now,
			Finalizers:        []string{metav1.FinalizerOrphanDependents},
		},
		Spec: runtimev1alpha1.WorkloadReplicaSetSpec{Replicas: &replicas},
	}
	rs.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp),
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleDown),
	)
	workload := func(name, generation string) *runtimev1alpha1.Workload {
		controller := true
		return &runtimev1alpha1.Workload{
			ObjectMeta: metav1.ObjectMeta{
				Namespace:   testNamespace,
				Name:        name,
				Annotations: map[string]string{workloadReplicaSetGenerationAnnotation: generation},
				OwnerReferences: []metav1.OwnerReference{{
					APIVersion: runtimev1alpha1.GroupVersion.String(),
					Kind:       replicaSetGvk.Kind,
					Name:       testReplicaSetName,
					UID:        rs.UID,
					Controller: &controller,
				}},
			},
		}
	}
	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(rs, workload("current", rs.Spec.Template.Hash()), workload("outdated", "an-old-template")).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, func(obj client.Object) []string {
			if ownerUID, ok := controllerUID(obj, replicaSetGvk); ok {
				return []string{ownerUID}
			}
			return nil
		}).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetHashIndex, func(obj client.Object) []string {
			return []string{obj.GetAnnotations()[workloadReplicaSetGenerationAnnotation]}
		}).
		Build()
	r := &WorkloadReplicaSetReconciler{Client: c, Scheme: s}
	ctx := context.Background()

	// One short of two would start another, and none would remove the current one.
	if err := r.reconcileScaleUp(ctx, rs); !errors.Is(err, condition.ErrNoop()) {
		t.Fatalf("reconcileScaleUp: expected a no-op, got %v", err)
	}
	rs.Spec.Replicas = &none
	if err := r.reconcileScaleDown(ctx, rs); !errors.Is(err, condition.ErrNoop()) {
		t.Fatalf("reconcileScaleDown: expected a no-op, got %v", err)
	}
	rs.Spec.Replicas = &replicas
	if err := r.reconcileCleanup(ctx, rs); err != nil {
		t.Fatalf("reconcileCleanup: %v", err)
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := c.List(ctx, workloads); err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(workloads.Items) != 2 {
		t.Fatalf("expected the Workloads unchanged, got %d", len(workloads.Items))
	}

	// A delete that lands mid-scale reports the same way.
	rs.Status.SetConditions(condition.UnknownCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp, "Reconcile", "scaling"))
	if err := r.reconcileReady(ctx, rs); !condition.IsStatusUnknown(err) {
		t.Fatalf("reconcileReady: expected Unknown, got %v", err)
	}
	if rs.Status.Replicas == nil || rs.Status.Replicas.Current != 1 {
		t.Fatalf("expected the current Workload counted, got %+v", rs.Status.Replicas)
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
			WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, replicaSetOwnerIndexValue(replicaSetGvk)).
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
