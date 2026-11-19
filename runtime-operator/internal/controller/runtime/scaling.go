package runtime

import (
	"context"
	"sync"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"k8s.io/apimachinery/pkg/util/wait"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

const (
	DefaultWorkloadConcurrency = 8
	DefaultCleanupConcurrency  = 16
	DefaultHostConcurrency     = 2
	DefaultReplicaBatchSize    = 64
	DefaultReplicaConcurrency  = 8
	busyRetry                  = 250 * time.Millisecond
	// busyRetryMax caps the backoff for requests waiting on a host's slots.
	// A released slot wakes the next waiter directly, so this is only the
	// fallback for a wake-up that was dropped, and a deep backlog (a start
	// holds its slot up to a minute) does not spin through the queue.
	busyRetryMax = 30 * time.Second
	// slotWakeBuffer bounds wake-ups not yet taken by the queue. A full buffer
	// drops the wake-up; the waiter's own requeue still runs.
	slotWakeBuffer = 1024
)

// busyBackoff spaces out retries for requests that found no free slot. The
// delay grows with how many requests wait on the same host, not with how long
// one request has waited, so newly queued requests do not outrun old ones.
type busyBackoff struct {
	mu      sync.Mutex
	waiting map[string]map[string]waiter
	hostOf  map[string]string
}

type waiter struct {
	req   reconcile.Request
	since time.Time
}

// retry records key as waiting on host, a slot pool, and returns its requeue.
// limit is the pool's slot count; key should be unique per queue.
func (b *busyBackoff) retry(host, key string, req reconcile.Request, limit int) reconcile.Result {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.waiting == nil {
		b.waiting = make(map[string]map[string]waiter)
		b.hostOf = make(map[string]string)
	}
	if previous, ok := b.hostOf[key]; ok && previous != host {
		b.forget(key)
	}
	if b.waiting[host] == nil {
		b.waiting[host] = make(map[string]waiter)
	}
	since := time.Now()
	if w, ok := b.waiting[host][key]; ok {
		since = w.since
	}
	b.waiting[host][key] = waiter{req: req, since: since}
	b.hostOf[key] = host
	// Each round frees about limit slots; spread the waiters over the rounds
	// ahead of them.
	rounds := (len(b.waiting[host]) + limit - 1) / max(1, limit)
	return reconcile.Result{RequeueAfter: jittered(min(busyRetry*time.Duration(rounds), busyRetryMax))}
}

// jittered adds up to 50% so a backlog does not wake in lockstep.
func jittered(delay time.Duration) time.Duration {
	return wait.Jitter(delay, 0.5)
}

// next removes and returns the longest-waiting request on host, so a released
// slot goes to it rather than to whichever retry happens to fire first.
func (b *busyBackoff) next(host string) (reconcile.Request, bool) {
	b.mu.Lock()
	defer b.mu.Unlock()
	var (
		oldest string
		found  waiter
	)
	for key, w := range b.waiting[host] {
		if oldest == "" || w.since.Before(found.since) {
			oldest, found = key, w
		}
	}
	if oldest == "" {
		return reconcile.Request{}, false
	}
	b.forget(oldest)
	return found.req, true
}

// done removes key from the waiters once it ran or no longer needs a slot.
func (b *busyBackoff) done(key string) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.forget(key)
}

func (b *busyBackoff) forget(key string) {
	host, ok := b.hostOf[key]
	if !ok {
		return
	}
	delete(b.hostOf, key)
	delete(b.waiting[host], key)
	if len(b.waiting[host]) == 0 {
		delete(b.waiting, host)
	}
}

// stalledVerdictKey carries withHostBudget's decision on a stalled host to
// finalize, so a cleanup looks the host up once.
type stalledVerdictKey struct{}

func (r *WorkloadReconciler) hostConcurrency() int {
	if r.HostConcurrency > 0 {
		return r.HostConcurrency
	}
	return DefaultHostConcurrency
}

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
		queue := "live/"
		if cleanup {
			queue = "cleanup/"
		}
		key := queue + req.String()
		// Read the object before taking its slot: a request this queue would
		// drop must not wait behind the other queue's hold on the object.
		workload := &runtimev1alpha1.Workload{}
		if err := r.Get(ctx, req.NamespacedName, workload); err != nil {
			r.busyWaits.done(key)
			return reconcile.Result{}, client.IgnoreNotFound(err)
		}
		if cleanup == workload.DeletionTimestamp.IsZero() {
			r.busyWaits.done(key)
			return reconcile.Result{}, nil
		}
		releaseObject, ok := r.objectSlots.acquire(req.String(), 1)
		if !ok {
			// The other queue holds this object only briefly; no backlog to
			// spread, so a short jittered retry is enough.
			return reconcile.Result{RequeueAfter: jittered(busyRetry)}, nil
		}
		defer releaseObject()
		host := workload.Status.HostID
		if host == "" {
			r.busyWaits.done(key)
			return next.Reconcile(ctx, req)
		}
		slots, pool := &r.liveSlots, host
		switch {
		case cleanup:
			slots, pool = &r.cleanupSlots, "cleanup/"+host
		case !workload.Status.AllTrue(runtimev1alpha1.WorkloadConditionPlacement):
			// A pass that may send a Start can hold its slot for the whole
			// start timeout. Starts get their own slots so they cannot keep
			// Sync polling and eviction for the host's other Workloads waiting.
			slots, pool = &r.startSlots, "start/"+host
		}
		release, ok := slots.acquire(host, r.hostConcurrency())
		if !ok {
			return r.busyWaits.retry(pool, key, req, r.hostConcurrency()), nil
		}
		defer func() {
			release()
			r.wakeWaiter(pool, cleanup)
		}()
		r.busyWaits.done(key)
		if cleanup && r.cleanupStalls.stalled(host) {
			// A gone or non-heartbeating host is dropped by finalize under the
			// existing hostUnavailable policy, without another stop timeout. A
			// Ready host that failed a stop keeps its finalizers and backs off.
			giveUp, err := r.stalledCleanupGivesUp(ctx, workload)
			if err != nil {
				return reconcile.Result{}, err
			}
			if !giveUp {
				return reconcile.Result{RequeueAfter: workloadEvictionStopTimeout}, nil
			}
			ctx = context.WithValue(ctx, stalledVerdictKey{}, true)
		}
		return next.Reconcile(ctx, req)
	})
}

// wakeWaiter hands a slot just released in pool to its longest waiter by
// queueing that request now, instead of leaving it to its backoff.
func (r *WorkloadReconciler) wakeWaiter(pool string, cleanup bool) {
	wake := r.liveWake
	if cleanup {
		wake = r.cleanupWake
	}
	if wake == nil {
		return
	}
	req, ok := r.busyWaits.next(pool)
	if !ok {
		return
	}
	w := &runtimev1alpha1.Workload{}
	w.Name, w.Namespace = req.Name, req.Namespace
	select {
	case wake <- event.GenericEvent{Object: w}:
	default:
	}
}
