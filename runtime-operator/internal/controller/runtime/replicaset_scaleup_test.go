package runtime

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func scaleUpReconciler(t *testing.T, funcs interceptor.Funcs, rs *runtimev1alpha1.WorkloadReplicaSet) (*WorkloadReplicaSetReconciler, *events.FakeRecorder) {
	t.Helper()
	s := newScaleTestScheme(t)
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}
	recorder := events.NewFakeRecorder(4)
	return &WorkloadReplicaSetReconciler{
		Client: fake.NewClientBuilder().
			WithScheme(s).
			WithObjects(rs).
			WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, replicaSetOwnerIndexValue(replicaSetGvk)).
			WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetRevisionIndex, replicaSetRevisionIndexValue(replicaSetGvk)).
			WithInterceptorFuncs(funcs).
			Build(),
		Scheme:   s,
		Recorder: recorder,
	}, recorder
}

// A ReplicaSet created directly has no Deployment to tell, so a refused
// create is recorded on the ReplicaSet alone.
func TestScaleUpRecordsRefusalOnStandaloneReplicaSet(t *testing.T) {
	rs := unavailableReplicaSet()
	rs.OwnerReferences = nil
	r, recorder := scaleUpReconciler(t, refuseCreates, rs)

	if err := r.reconcileScaleUp(context.Background(), rs); err == nil {
		t.Fatal("scale up succeeded despite the refusal")
	}
	got := drainEvents(recorder)
	if len(got) != 1 || !strings.HasPrefix(got[0], "Warning FailedCreate") {
		t.Fatalf("events = %q, want one on the ReplicaSet", got)
	}
}

// A create that collides with an existing Workload reads it past the cache to
// decide whether it is this ReplicaSet's; a read that fails fails the pass.
func TestScaleUpCollisionReadFailure(t *testing.T) {
	rs := unavailableReplicaSet()
	r, _ := scaleUpReconciler(t, interceptor.Funcs{
		Create: func(_ context.Context, _ client.WithWatch, obj client.Object, _ ...client.CreateOption) error {
			return apierrors.NewAlreadyExists(schema.GroupResource{Group: runtimev1alpha1.GroupVersion.Group, Resource: workloadResource}, obj.GetName())
		},
	}, rs)
	r.APIReader = failingReader{}

	err := r.reconcileScaleUp(context.Background(), rs)
	if !errors.Is(err, errUncachedRead) {
		t.Fatalf("scale up returned %v, want the read error", err)
	}
	if errors.Is(err, condition.ErrNoop()) {
		t.Fatal("a failed read was treated as a cache lag")
	}
}

// A Workload already being deleted is on its way out; counting it as failed
// would start a second replacement for the same replica.
func TestFailedPastGracePeriodSkipsDeletingWorkload(t *testing.T) {
	workload := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{DeletionTimestamp: &metav1.Time{Time: time.Now()}}}
	failed := condition.ErrorCondition(runtimev1alpha1.WorkloadConditionSync, "Reconcile", errors.New("host gone"))
	failed.LastTransitionTime = metav1.NewTime(time.Now().Add(-time.Hour))
	workload.Status.SetConditions(failed)

	if isFailedPastGracePeriod(workload, time.Time{}) {
		t.Fatal("a deleting workload was reported as failed past its grace period")
	}
	workload.DeletionTimestamp = nil
	if !isFailedPastGracePeriod(workload, time.Time{}) {
		t.Fatal("the same workload not being deleted was not reported as failed")
	}
}
