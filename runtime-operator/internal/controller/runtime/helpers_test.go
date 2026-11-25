package runtime

import (
	"context"
	"errors"
	"testing"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// Names the tests share: a Workload's namespace and Environment, another
// tenant's, and the usual Workload names.
const (
	testTenant  = "tenant-a"
	otherTenant = "tenant-b"
	placedName  = "placed"
	pinnedName  = "pinned"
)

// refuseCreates fails every Create the way an admission webhook denial does.
var refuseCreates = interceptor.Funcs{
	Create: func(_ context.Context, _ client.WithWatch, obj client.Object, _ ...client.CreateOption) error {
		return apierrors.NewForbidden(schema.GroupResource{Group: runtimev1alpha1.GroupVersion.Group},
			obj.GetName(), errors.New("denied by policy require-team-label: "+obj.GetName()+" has no team"))
	},
}

// drainEvents returns every event recorded so far.
func drainEvents(recorder *events.FakeRecorder) []string {
	var got []string
	for {
		select {
		case event := <-recorder.Events:
			got = append(got, event)
		default:
			return got
		}
	}
}

// place runs placement as the reconciler's requeue would: a pass that only
// records the Workload's owner is followed at once by the one that starts it.
func place(ctx context.Context, r *WorkloadReconciler, workload *runtimev1alpha1.Workload) error {
	before := workload.Status.Owner
	err := r.reconcilePlacement(ctx, workload)
	if errors.Is(err, condition.ErrNoop()) && workload.Status.Owner != before {
		return r.reconcilePlacement(ctx, workload)
	}
	return err
}

// recordOwner runs the placement pass that records the Workload's owner, so
// a test can compare what the start that follows changes.
func recordOwner(t *testing.T, r *WorkloadReconciler, workload *runtimev1alpha1.Workload) {
	t.Helper()
	if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, condition.ErrNoop()) || workload.Status.Owner == nil {
		t.Fatalf("recording the owner = %v, owner %v", err, workload.Status.Owner)
	}
}
