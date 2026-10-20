package runtime

import (
	"context"
	"errors"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

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
