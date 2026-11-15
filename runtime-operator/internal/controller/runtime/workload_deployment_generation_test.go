package runtime

import (
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// newPipelineReconciler runs the real condition pipeline on a fake client
// that keeps status writes apart from spec writes, as the API server does.
func newPipelineReconciler(t *testing.T, objs ...client.Object) *WorkloadDeploymentReconciler {
	t.Helper()
	s := newScaleTestScheme(t)
	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithStatusSubresource(&runtimev1alpha1.WorkloadDeployment{}, &runtimev1alpha1.WorkloadReplicaSet{}).
		WithIndex(&runtimev1alpha1.WorkloadReplicaSet{}, workloadDeploymentOwnerIndex, replicaSetOwnerIndex(t, s)).
		Build()
	r := &WorkloadDeploymentReconciler{Client: c, Scheme: s}
	r.reconciler = r.newReconciler()
	return r
}

// reconcilePass runs one pass and returns the deployment as stored.
func reconcilePass(t *testing.T, r *WorkloadDeploymentReconciler, key types.NamespacedName) *runtimev1alpha1.WorkloadDeployment {
	t.Helper()
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}
	wd := &runtimev1alpha1.WorkloadDeployment{}
	if err := r.Get(t.Context(), key, wd); err != nil {
		t.Fatal(err)
	}
	return wd
}

func currentReplicaSet(t *testing.T, r *WorkloadDeploymentReconciler, wd *runtimev1alpha1.WorkloadDeployment) *runtimev1alpha1.WorkloadReplicaSet {
	t.Helper()
	if wd.Status.CurrentReplicaSet == nil {
		return nil
	}
	rs := &runtimev1alpha1.WorkloadReplicaSet{}
	key := types.NamespacedName{Namespace: wd.Namespace, Name: wd.Status.CurrentReplicaSet.Name}
	if err := r.Get(t.Context(), key, rs); err != nil {
		t.Fatal(err)
	}
	return rs
}

// markReplicaSetAvailable does what the ReplicaSet controller does once the
// set's workloads run: every condition True for the set's generation.
func markReplicaSetAvailable(t *testing.T, r *WorkloadDeploymentReconciler, wd *runtimev1alpha1.WorkloadDeployment) {
	t.Helper()
	rs := currentReplicaSet(t, r, wd)
	if rs == nil {
		return
	}
	for _, ct := range []condition.ConditionType{
		runtimev1alpha1.WorkloadReplicaSetConditionScaleUp,
		runtimev1alpha1.WorkloadReplicaSetConditionScaleDown,
		condition.TypeReady,
	} {
		c := condition.ReadyCondition(ct)
		c.ObservedGeneration = rs.Generation
		rs.Status.SetConditions(c)
	}
	rs.Status.Replicas = &runtimev1alpha1.ReplicaSetStatus{Expected: testReplicas, Current: testReplicas, Ready: testReplicas}
	if err := r.Status().Update(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
}

// markReplicaSetStarted is the ReplicaSet controller's first pass on a new set:
// it records one condition per pass, ScaleUp first.
func markReplicaSetStarted(t *testing.T, r *WorkloadDeploymentReconciler, wd *runtimev1alpha1.WorkloadDeployment) {
	t.Helper()
	rs := currentReplicaSet(t, r, wd)
	if rs == nil {
		return
	}
	rs.Generation = 1
	if err := r.Update(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
	c := condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp)
	c.ObservedGeneration = rs.Generation
	rs.Status.SetConditions(c)
	if err := r.Status().Update(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
}

// status.observedGeneration reaches a generation only once Ready is found True
// for it, so `kubectl wait --for=condition=Ready` does not accept a Ready left
// from before a template edit.
func TestDeploymentObservedGenerationFollowsReady(t *testing.T) {
	replicas := testReplicas
	wd := &runtimev1alpha1.WorkloadDeployment{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: testDeploymentName, UID: testDeploymentUID, Generation: 1},
		Spec: runtimev1alpha1.WorkloadDeploymentSpec{
			WorkloadReplicaSetSpec: runtimev1alpha1.WorkloadReplicaSetSpec{Replicas: &replicas},
			DeployPolicy:           runtimev1alpha1.WorkloadDeployPolicyRollingUpdate,
		},
	}
	r := newPipelineReconciler(t, wd)
	key := client.ObjectKeyFromObject(wd)

	var got *runtimev1alpha1.WorkloadDeployment
	for range 8 {
		got = reconcilePass(t, r, key)
		markReplicaSetAvailable(t, r, got)
	}
	if !got.Status.IsAvailable() || got.Status.ObservedGeneration != 1 {
		t.Fatalf("after the first rollout: ready=%v observedGeneration=%d, want true and 1",
			got.Status.IsAvailable(), got.Status.ObservedGeneration)
	}
	firstSet := got.Status.CurrentReplicaSet.Name

	got.Generation = 2
	got.Spec.Template.Spec.HostSelector = map[string]string{"edited": "true"}
	if err := r.Update(t.Context(), got); err != nil {
		t.Fatal(err)
	}
	// The edited template's set is not available yet, so its generation must
	// not be reported, whatever Ready still says.
	for pass := range 6 {
		got = reconcilePass(t, r, key)
		if got.Status.ObservedGeneration == got.Generation {
			t.Fatalf("pass %d: observedGeneration reached %d before the edited template's set was available",
				pass, got.Generation)
		}
		if got.Status.CurrentReplicaSet.Name != firstSet {
			markReplicaSetStarted(t, r, got)
		}
	}
	if got.Status.CurrentReplicaSet.Name == firstSet {
		t.Fatal("the edit made no new ReplicaSet")
	}
	for range 6 {
		got = reconcilePass(t, r, key)
		markReplicaSetAvailable(t, r, got)
	}
	if !got.Status.IsAvailable() || got.Status.ObservedGeneration != 2 || got.Status.CurrentReplicaSet.Name == firstSet {
		t.Fatalf("after the edit rolled out: ready=%v observedGeneration=%d set=%s, want true, 2 and a new set",
			got.Status.IsAvailable(), got.Status.ObservedGeneration, got.Status.CurrentReplicaSet.Name)
	}

	// A replicas-only edit, as from an autoscaler, changes no condition: Ready
	// stays True and still reports the new generation.
	got.Generation = 3
	more := testReplicas + 1
	got.Spec.Replicas = &more
	if err := r.Update(t.Context(), got); err != nil {
		t.Fatal(err)
	}
	for pass := range 3 {
		if got = reconcilePass(t, r, key); !got.Status.IsAvailable() {
			t.Fatalf("pass %d: a replicas-only edit took Ready from True", pass)
		}
	}
	if got.Status.ObservedGeneration != 3 {
		t.Fatalf("observedGeneration = %d after a replicas-only edit, want 3", got.Status.ObservedGeneration)
	}
}
