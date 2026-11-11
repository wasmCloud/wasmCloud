package runtime

import (
	"context"
	"sync"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

const (
	DefaultWorkloadConcurrency = 8
	DefaultCleanupConcurrency  = 16
	DefaultHostConcurrency     = 2
	DefaultReplicaBatchSize    = 64
	DefaultReplicaConcurrency  = 8
	busyRetry                  = 250 * time.Millisecond
)

// hostSlots never waits while holding a controller worker. Empty entries are
// removed as soon as their last operation finishes.
type hostSlots struct {
	mu     sync.Mutex
	active map[string]int
}

func (s *hostSlots) acquire(host string, limit int) (func(), bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.active == nil {
		s.active = make(map[string]int)
	}
	if s.active[host] >= limit {
		return nil, false
	}
	s.active[host]++
	return func() {
		s.mu.Lock()
		defer s.mu.Unlock()
		s.active[host]--
		if s.active[host] == 0 {
			delete(s.active, host)
		}
	}, true
}

func (r *WorkloadReconciler) withHostBudget(next reconcile.Reconciler, cleanup bool) reconcile.Reconciler {
	return reconcile.Func(func(ctx context.Context, req reconcile.Request) (reconcile.Result, error) {
		releaseObject, ok := r.objectSlots.acquire(req.String(), 1)
		if !ok {
			return reconcile.Result{RequeueAfter: busyRetry}, nil
		}
		defer releaseObject()
		workload := &runtimev1alpha1.Workload{}
		if err := r.Get(ctx, req.NamespacedName, workload); err != nil {
			return reconcile.Result{}, client.IgnoreNotFound(err)
		}
		if cleanup == workload.DeletionTimestamp.IsZero() {
			return reconcile.Result{}, nil
		}
		host := workload.Status.HostID
		if host == "" {
			return next.Reconcile(ctx, req)
		}
		slots := &r.liveSlots
		if cleanup {
			slots = &r.cleanupSlots
		}
		release, ok := slots.acquire(host, max(1, r.HostConcurrency))
		if !ok {
			return reconcile.Result{RequeueAfter: busyRetry}, nil
		}
		defer release()
		if cleanup && r.cleanupStalls.stalled(host) {
			// Only a gone or explicitly failed host can use the existing give-up
			// policy without another stop timeout. Unknown/Ready hosts retain their
			// finalizers while backing off.
			current, err := r.lookupHostByID(ctx, host)
			if err != nil {
				return reconcile.Result{}, err
			}
			if !hostFailed(current) {
				return reconcile.Result{RequeueAfter: workloadEvictionStopTimeout}, nil
			}
		}
		return next.Reconcile(ctx, req)
	})
}
