package runtime

import (
	"context"
	"errors"
	"testing"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	testNamespace      = "wasmcloud-system"
	testDeploymentName = "messaging-echo"
	testReplicaSetName = "messaging-echo-current"
	testPrevSetName    = "messaging-echo-prev"
)

func newScaleTestScheme(t *testing.T) *runtime.Scheme {
	t.Helper()
	s := runtime.NewScheme()
	if err := corev1.AddToScheme(s); err != nil {
		t.Fatalf("add corev1: %v", err)
	}
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	return s
}

// readyDeployment returns a WD already past Sync+Deploy so reconcileScale
// will run all the way through to the IsAvailable()..
func readyDeployment(prevSetName string) *runtimev1alpha1.WorkloadDeployment {
	replicas := int32(1)
	wd := &runtimev1alpha1.WorkloadDeployment{
		TypeMeta: metav1.TypeMeta{
			APIVersion: "runtime.wasmcloud.dev/v1alpha1",
			Kind:       "WorkloadDeployment",
		},
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testNamespace,
			Name:      testDeploymentName,
		},
		Spec: runtimev1alpha1.WorkloadDeploymentSpec{
			WorkloadReplicaSetSpec: runtimev1alpha1.WorkloadReplicaSetSpec{
				Replicas: &replicas,
			},
			DeployPolicy: runtimev1alpha1.WorkloadDeployPolicyRollingUpdate,
		},
		Status: runtimev1alpha1.WorkloadDeploymentStatus{
			CurrentReplicaSet: &corev1.LocalObjectReference{Name: testReplicaSetName},
		},
	}
	if prevSetName != "" {
		wd.Status.PreviousReplicaSet = &corev1.LocalObjectReference{Name: prevSetName}
	}
	wd.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionSync),
		condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionDeploy),
	)
	return wd
}

// All RS fixtures use replicas=1; the gate logic under test doesn't care
// about the count. If a future test needs N>1, add a parallel helper.
const testReplicas = int32(1)

// baseReplicaSet builds the shared scaffolding (TypeMeta, owner ref, Spec)
// that every variant shares; the available/unavailable/uninitialized
// helpers below layer the relevant Status onto it.
func baseReplicaSet(name string) *runtimev1alpha1.WorkloadReplicaSet {
	r := testReplicas
	return &runtimev1alpha1.WorkloadReplicaSet{
		TypeMeta: metav1.TypeMeta{
			APIVersion: "runtime.wasmcloud.dev/v1alpha1",
			Kind:       "WorkloadReplicaSet",
		},
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testNamespace,
			Name:      name,
			OwnerReferences: []metav1.OwnerReference{{
				APIVersion: "runtime.wasmcloud.dev/v1alpha1",
				Kind:       "WorkloadDeployment",
				Name:       testDeploymentName,
				Controller: ptrTrue(),
				UID:        "wd-uid",
			}},
		},
		Spec: runtimev1alpha1.WorkloadReplicaSetSpec{
			Replicas: &r,
		},
	}
}

// availableReplicaSet returns an RS whose Ready condition is True and whose
// Status.Replicas reports all replicas ready.
func availableReplicaSet(name string) *runtimev1alpha1.WorkloadReplicaSet {
	rs := baseReplicaSet(name)
	rs.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	rs.Status.Replicas = &runtimev1alpha1.ReplicaSetStatus{
		Expected: testReplicas,
		Current:  testReplicas,
		Ready:    testReplicas,
	}
	return rs
}

// unavailableReplicaSet returns an RS that has been reconciled at least once
// but whose workloads are not yet running (Status.Replicas reports
// Unavailable, Ready condition is absent).
func unavailableReplicaSet(name string) *runtimev1alpha1.WorkloadReplicaSet {
	rs := baseReplicaSet(name)
	rs.Status.Replicas = &runtimev1alpha1.ReplicaSetStatus{
		Expected:    testReplicas,
		Current:     testReplicas,
		Ready:       0,
		Unavailable: testReplicas,
	}
	return rs
}

// uninitializedReplicaSet returns an RS that has been created but never
// reconciled — Status.Replicas is nil and no conditions are set. This is
// the actual state observed in production immediately after the Deploy
// step creates the RS; reconcileScale must treat it as not-yet-available.
func uninitializedReplicaSet(name string) *runtimev1alpha1.WorkloadReplicaSet {
	return baseReplicaSet(name)
}

func ptrTrue() *bool { b := true; return &b }

func newScaleReconciler(t *testing.T, objs ...client.Object) *WorkloadDeploymentReconciler {
	t.Helper()
	s := newScaleTestScheme(t)

	// Mirror SetupWithManager's index, reconcileScale's List() call uses
	// it to find sibling ReplicaSets to clean up.
	deploymentGvk, err := gvkForType(&runtimev1alpha1.WorkloadDeployment{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}

	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.WorkloadReplicaSet{}, workloadDeploymentNameIndex,
			func(obj client.Object) []string {
				if name, ok := isOwnedByController(obj, deploymentGvk); ok {
					return []string{name}
				}
				return nil
			}).
		Build()

	return &WorkloadDeploymentReconciler{Client: c, Scheme: s}
}

// Regression coverage for reconcileScale used to return nil on a fresh
// deploy regardless of whether the active ReplicaSet had any running
// workloads, which propagated up to Ready=True before the workload had
// been placed on a host. The fix gates Scale on the current RS being
// available; these tests pin that contract.
func TestReconcileScale_FreshDeploy_RSNotAvailable_ReturnsUnknown(t *testing.T) {
	// The bug: a fresh deploy used to return nil here regardless of
	// replica health, so Scale=True propagated to Ready=True even though
	// the underlying Workload was still unplaced. Lock in that an RS
	// with Unavailable=1 keeps reconcileScale in the Unknown path.
	wd := readyDeployment("" /* no previous RS */)
	rs := unavailableReplicaSet(testReplicaSetName)
	r := newScaleReconciler(t, wd, rs)

	err := r.reconcileScale(context.Background(), wd)
	if err == nil {
		t.Fatalf("expected error from reconcileScale, got nil (Scale would be set True)")
	}
	if !condition.IsStatusUnknown(err) {
		t.Fatalf("expected ErrStatusUnknown wrapping, got %v", err)
	}
	// Status.Replicas should still be populated even on the unavailable path,
	// so the WD reflects what the operator observed.
	if wd.Status.Replicas == nil {
		t.Fatalf("expected Status.Replicas to be set")
	}
	if got := wd.Status.Replicas.Unavailable; got != 1 {
		t.Fatalf("expected Unavailable=1 in status, got %d", got)
	}
}

// In production an RS exists for several reconcile cycles before
// WorkloadReplicaSet.Status.Replicas is populated — the controller has to
// observe the children, summarize them, and patch status. Until that
// happens IsAvailable() is false (Ready condition absent), and reconcileScale
// must not let Scale=True through. Without this case, a refactor that
// special-cased "Status.Replicas != nil" to skip the gate would pass the
// other tests but ship a regression for newly-created RSes.
func TestReconcileScale_FreshDeploy_RSStatusUninitialized_ReturnsUnknown(t *testing.T) {
	wd := readyDeployment("")
	rs := uninitializedReplicaSet(testReplicaSetName)
	r := newScaleReconciler(t, wd, rs)

	err := r.reconcileScale(context.Background(), wd)
	if err == nil {
		t.Fatalf("expected error from reconcileScale, got nil")
	}
	if !condition.IsStatusUnknown(err) {
		t.Fatalf("expected ErrStatusUnknown wrapping, got %v", err)
	}
}

func TestReconcileScale_FreshDeploy_RSAvailable_ReturnsNil(t *testing.T) {
	// Once the RS is Available the gate must let through, otherwise
	// no deploy ever reaches Ready=True.
	wd := readyDeployment("")
	rs := availableReplicaSet(testReplicaSetName)
	r := newScaleReconciler(t, wd, rs)

	if err := r.reconcileScale(context.Background(), wd); err != nil {
		t.Fatalf("expected nil error from reconcileScale, got %v", err)
	}
	if wd.Status.Replicas == nil || wd.Status.Replicas.Ready != 1 {
		t.Fatalf("expected Status.Replicas.Ready=1, got %+v", wd.Status.Replicas)
	}
}

func TestReconcileScale_RollingUpdate_CurrentNotAvailable_ReturnsUnknown(t *testing.T) {
	// Same gate, but with a previous RS still around. Asserts the prev RS
	// is NOT deleted while current is unavailable, otherwise a rolling
	// update would tear down the old workload before the new one is
	// running, causing the downtime that RollingUpdate is supposed to
	// prevent.
	wd := readyDeployment(testPrevSetName)
	current := unavailableReplicaSet(testReplicaSetName)
	prev := availableReplicaSet(testPrevSetName)
	r := newScaleReconciler(t, wd, current, prev)

	err := r.reconcileScale(context.Background(), wd)
	if err == nil {
		t.Fatalf("expected error from reconcileScale, got nil")
	}
	if !condition.IsStatusUnknown(err) {
		t.Fatalf("expected ErrStatusUnknown, got %v", err)
	}

	got := &runtimev1alpha1.WorkloadReplicaSet{}
	if err := r.Get(context.Background(),
		client.ObjectKey{Namespace: testNamespace, Name: testPrevSetName}, got); err != nil {
		t.Fatalf("previous ReplicaSet was unexpectedly deleted: %v", err)
	}
}

// Happy path completion of a rolling update: current is available, the
// previous RS must be deleted and the function must short-circuit via
// ErrSkipReconciliation so the conditioned reconciler sets Scale=True
// without falling through to the post-loop nil-return. Pins the cleanup
// behaviour so a refactor can't accidentally leave a stale RS behind.
func TestReconcileScale_RollingUpdate_CurrentAvailable_DeletesPrev(t *testing.T) {
	wd := readyDeployment(testPrevSetName)
	current := availableReplicaSet(testReplicaSetName)
	prev := availableReplicaSet(testPrevSetName)
	r := newScaleReconciler(t, wd, current, prev)

	err := r.reconcileScale(context.Background(), wd)
	if err == nil {
		t.Fatalf("expected ErrSkipReconciliation from reconcileScale, got nil")
	}
	if !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatalf("expected ErrSkipReconciliation, got %v", err)
	}

	if wd.Status.PreviousReplicaSet != nil {
		t.Fatalf("expected PreviousReplicaSet to be cleared, got %+v", wd.Status.PreviousReplicaSet)
	}
	got := &runtimev1alpha1.WorkloadReplicaSet{}
	getErr := r.Get(context.Background(),
		client.ObjectKey{Namespace: testNamespace, Name: testPrevSetName}, got)
	if !apierrors.IsNotFound(getErr) {
		t.Fatalf("expected previous ReplicaSet to be deleted (NotFound), got err=%v obj=%+v", getErr, got)
	}
}
