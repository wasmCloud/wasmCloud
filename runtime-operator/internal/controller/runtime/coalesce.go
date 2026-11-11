package runtime

import (
	"context"
	"reflect"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"k8s.io/client-go/util/workqueue"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

// AddAfter retains the earliest deadline for a key: a continuous event stream
// cannot postpone reconciliation indefinitely. No per-object timer is retained.
type coalescingQueue struct {
	workqueue.TypedRateLimitingInterface[reconcile.Request]
}

func (q coalescingQueue) Add(req reconcile.Request) { q.AddAfter(req, 100*time.Millisecond) }

func coalesce(h handler.EventHandler) handler.EventHandler {
	return handler.Funcs{
		CreateFunc: func(ctx context.Context, e event.CreateEvent, q workqueue.TypedRateLimitingInterface[reconcile.Request]) {
			h.Create(ctx, e, coalescingQueue{q})
		},
		UpdateFunc: func(ctx context.Context, e event.UpdateEvent, q workqueue.TypedRateLimitingInterface[reconcile.Request]) {
			h.Update(ctx, e, coalescingQueue{q})
		},
		DeleteFunc: func(ctx context.Context, e event.DeleteEvent, q workqueue.TypedRateLimitingInterface[reconcile.Request]) {
			h.Delete(ctx, e, coalescingQueue{q})
		},
		GenericFunc: func(ctx context.Context, e event.GenericEvent, q workqueue.TypedRateLimitingInterface[reconcile.Request]) {
			h.Generic(ctx, e, coalescingQueue{q})
		},
	}
}

// Ignore finalizer-only and resourceVersion-only child updates. Preserve all
// status, ownership, revision, and deletion changes consumed by parent logic.
func childChanges() predicate.Predicate {
	return predicate.Funcs{UpdateFunc: func(e event.UpdateEvent) bool {
		a, b := e.ObjectOld, e.ObjectNew
		if a == nil || b == nil {
			return true
		}
		if a.GetUID() != b.GetUID() || a.GetGeneration() != b.GetGeneration() ||
			!reflect.DeepEqual(a.GetDeletionTimestamp(), b.GetDeletionTimestamp()) ||
			!reflect.DeepEqual(a.GetOwnerReferences(), b.GetOwnerReferences()) ||
			!reflect.DeepEqual(a.GetAnnotations(), b.GetAnnotations()) ||
			!reflect.DeepEqual(a.GetLabels(), b.GetLabels()) {
			return true
		}
		return !reflect.DeepEqual(childStatus(a), childStatus(b))
	}}
}

func childStatus(obj client.Object) any {
	switch o := obj.(type) {
	case *runtimev1alpha1.Workload:
		return o.Status
	case *runtimev1alpha1.WorkloadReplicaSet:
		return o.Status
	default:
		return obj
	}
}
