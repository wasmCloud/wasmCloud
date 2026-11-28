package runtime

import (
	"context"
	"errors"
	"fmt"
	"sync/atomic"
	"testing"
	"testing/synctest"
	"time"

	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	corev1 "k8s.io/api/core/v1"
	discoveryv1 "k8s.io/api/discovery/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

func TestReplicaBatchesYieldAndPreserveTerminatingVictims(t *testing.T) {
	rs := cleanupReplicaSet()
	rs.Spec.Replicas = new(int32(10))
	r := newCleanupReconciler(t, interceptor.Funcs{}, rs)
	r.BatchSize, r.Concurrency, r.APIReader = 3, 2, r.Client
	// ScaleUp stays open while replicas are missing, so cleanup of previous
	// revisions, which waits on it, cannot run mid-rollout.
	if err := r.reconcileScaleUp(t.Context(), rs); !condition.IsStatusUnknown(err) {
		t.Fatalf("partial scale-up reported %v", err)
	}
	list := &runtimev1alpha1.WorkloadList{}
	if err := r.List(t.Context(), list); err != nil {
		t.Fatal(err)
	}
	if len(list.Items) != 3 {
		t.Fatalf("created %d replicas in a batch of 3", len(list.Items))
	}
	for _, w := range list.Items {
		if len(w.Finalizers) != 1 || w.Finalizers[0] != workloadFinalizerName {
			t.Fatal("missing creation-time finalizer")
		}
	}
	rs.Spec.Replicas = new(int32(1))
	if err := r.Update(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
	for range 3 {
		if err := r.reconcileScaleDown(t.Context(), rs); err != nil {
			t.Fatal(err)
		}
	}
	if err := r.List(t.Context(), list); err != nil {
		t.Fatal(err)
	}
	live := 0
	for _, w := range list.Items {
		if w.DeletionTimestamp.IsZero() {
			live++
		}
	}
	if live != 1 || len(list.Items) != 3 {
		t.Fatalf("live=%d total=%d; repeated scale-down selected extra victims", live, len(list.Items))
	}
}

func TestReplicaCleanupDeletesOneBatchPerPassAndRequeuesOnChangedIntent(t *testing.T) {
	rs := cleanupReplicaSet()
	rs.Spec.Replicas = new(int32(5))
	objects := []client.Object{rs}
	for i := range 5 {
		previous := cleanupWorkload(rs, fmt.Sprintf("previous-%d", i))
		previous.Annotations[workloadReplicaSetGenerationAnnotation] = cleanupPreviousHash
		objects = append(objects, previous, failedCleanupWorkload(rs, fmt.Sprintf("failed-%d", i), time.Now().Add(-time.Hour)))
	}
	var deletes atomic.Int32
	r := newCleanupReconciler(t, interceptor.Funcs{
		Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
			deletes.Add(1)
			return c.Delete(ctx, obj, opts...)
		},
	}, objects...)
	r.BatchSize, r.Concurrency, r.APIReader = 3, 2, r.Client
	if err := r.reconcileCleanup(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
	if n := deletes.Load(); n != 3 {
		t.Fatalf("one cleanup pass issued %d deletes with a batch of 3", n)
	}
	// The ReplicaSet changes before the next batch: the post-hook requeues
	// rather than failing the reconcile.
	changed := rs.DeepCopy()
	changed.Spec.Replicas = new(int32(6))
	if err := r.Update(t.Context(), changed); err != nil {
		t.Fatal(err)
	}
	if err := r.reconcileCleanup(t.Context(), rs); err != nil {
		t.Fatalf("changed intent failed cleanup: %v", err)
	}
	if n := deletes.Load(); n != 3 {
		t.Fatalf("cleanup deleted against a stale ReplicaSet: %d deletes", n)
	}
}

// A delete guarded by a resourceVersion from a stale cache conflicts. A victim
// the API shows already deleting counts as deleted; one that changed while
// live is spared and the pass requeues, instead of ScaleDown failing.
func TestReplicaScaleDownToleratesConflictsFromAStaleCache(t *testing.T) {
	for _, tc := range []struct {
		name        string
		liveChanged bool
	}{{"already deleting", false}, {"changed while live", true}} {
		t.Run(tc.name, func(t *testing.T) {
			rs := cleanupReplicaSet()
			rs.Spec.Replicas = new(int32(0))
			victim := cleanupWorkload(rs, "victim")
			victim.UID = "victim-uid"
			fresh := victim.DeepCopy()
			fresh.ResourceVersion = ""
			if !tc.liveChanged {
				now := metav1.Now()
				fresh.DeletionTimestamp = &now
				fresh.Finalizers = []string{workloadFinalizerName}
			}
			r := newCleanupReconciler(t, interceptor.Funcs{
				Delete: func(context.Context, client.WithWatch, client.Object, ...client.DeleteOption) error {
					return apierrors.NewConflict(schema.GroupResource{Resource: "workloads"}, victim.Name, errors.New("stale resourceVersion"))
				},
			}, rs, victim)
			r.APIReader = fake.NewClientBuilder().WithScheme(r.Scheme).WithObjects(rs.DeepCopy(), fresh).Build()
			err := r.reconcileScaleDown(t.Context(), rs)
			if tc.liveChanged {
				if !condition.IsStatusUnknown(err) {
					t.Fatalf("a changed live victim reported %v, want a retry", err)
				}
				return
			}
			if err != nil {
				t.Fatalf("an already-deleting victim failed scale-down: %v", err)
			}
		})
	}
}

// When the excess spans batches, the first batch still takes the unavailable
// replica, not the lowest-named healthy one.
func TestReplicaScaleDownBatchTakesUnavailableFirst(t *testing.T) {
	rs := cleanupReplicaSet()
	rs.Spec.Replicas = new(int32(1))
	var objects []client.Object
	objects = append(objects, rs)
	for _, name := range []string{"a-good", "b-good"} {
		w := cleanupWorkload(rs, name)
		w.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
		objects = append(objects, w)
	}
	bad := failedCleanupWorkload(rs, "c-bad", time.Now())
	objects = append(objects, bad)
	r := newCleanupReconciler(t, interceptor.Funcs{}, objects...)
	r.BatchSize = 1
	if err := r.reconcileScaleDown(t.Context(), rs); !condition.IsStatusUnknown(err) {
		t.Fatalf("partial scale-down reported %v", err)
	}
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(bad), &runtimev1alpha1.Workload{}); !apierrors.IsNotFound(err) {
		t.Fatalf("first batch kept the unavailable replica: %v", err)
	}
}

func TestDroppedSlotWakeKeepsTheWaitersPlace(t *testing.T) {
	r := &WorkloadReconciler{liveWake: make(chan event.GenericEvent, 1)}
	r.liveWake <- event.GenericEvent{} // the wake buffer is full
	oldest := reconcile.Request{NamespacedName: client.ObjectKey{Namespace: testNamespace, Name: "oldest"}}
	newer := reconcile.Request{NamespacedName: client.ObjectKey{Namespace: testNamespace, Name: "newer"}}
	r.busyWaits.retry("pool", "oldest", oldest, 1)
	time.Sleep(time.Millisecond)
	r.busyWaits.retry("pool", "newer", newer, 1)
	r.wakeWaiter("pool", false)
	// The oldest waiter's own requeue fires: it keeps its original place.
	r.busyWaits.retry("pool", "oldest", oldest, 1)
	<-r.liveWake
	r.wakeWaiter("pool", false)
	select {
	case e := <-r.liveWake:
		if e.Object.GetName() != oldest.Name {
			t.Fatalf("woke %s before the longest waiter", e.Object.GetName())
		}
	default:
		t.Fatal("a released slot did not wake a waiter")
	}
}

func TestReplicaScaleDownRemovesUnavailableReplicasFirst(t *testing.T) {
	rs := cleanupReplicaSet()
	rs.Spec.Replicas = new(int32(1))
	bad := failedCleanupWorkload(rs, "a-bad", time.Now())
	good := cleanupWorkload(rs, "b-good")
	good.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	r := newCleanupReconciler(t, interceptor.Funcs{}, rs, bad, good)
	if err := r.reconcileScaleDown(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(good), &runtimev1alpha1.Workload{}); err != nil {
		t.Fatalf("scale-down removed the available replica: %v", err)
	}
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(bad), &runtimev1alpha1.Workload{}); !apierrors.IsNotFound(err) {
		t.Fatalf("scale-down kept the unavailable replica: %v", err)
	}
	// A negative replica count scales to zero instead of panicking.
	rs.Spec.Replicas = new(int32(-1))
	if err := r.reconcileScaleDown(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(good), &runtimev1alpha1.Workload{}); !apierrors.IsNotFound(err) {
		t.Fatalf("negative replicas kept a replica: %v", err)
	}
}

func TestReplicaBatchRejectsReversedScaleDecision(t *testing.T) {
	rs := cleanupReplicaSet()
	rs.Spec.Replicas = new(int32(0))
	w := cleanupWorkload(rs, "survivor")
	r := newCleanupReconciler(t, interceptor.Funcs{}, rs, w)
	r.APIReader = r.Client
	current := rs.DeepCopy()
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(rs), current); err != nil {
		t.Fatal(err)
	}
	current.Spec.Replicas = new(int32(2))
	if err := r.Update(t.Context(), current); err != nil {
		t.Fatal(err)
	}
	if err := r.reconcileScaleDown(t.Context(), rs); !condition.IsStatusUnknown(err) {
		t.Fatalf("stale scale decision: %v", err)
	}
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(w), &runtimev1alpha1.Workload{}); err != nil {
		t.Fatal("stale scale-down deleted the survivor", err)
	}
}

func TestHostBudgetsYieldWithoutBlockingOtherHostsOrLiveWork(t *testing.T) {
	deleting := placedWorkload()
	deleting.Name = "budget-deleting"
	deleting.Finalizers = []string{workloadFinalizerName}
	now := metav1.Now()
	deleting.DeletionTimestamp = &now
	live := placedWorkload()
	live.Name = "budget-live"
	r := &WorkloadReconciler{Client: newWorkloadClient(t, deleting, live), HostConcurrency: 1}
	release, ok := r.cleanupSlots.acquire(deleting.Status.HostID, 1)
	if !ok {
		t.Fatal("cannot occupy cleanup slot")
	}
	defer release()
	var calls atomic.Int32
	next := reconcile.Func(func(context.Context, reconcile.Request) (reconcile.Result, error) {
		calls.Add(1)
		return reconcile.Result{}, nil
	})
	result, err := r.withHostBudget(next, true).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(deleting)})
	if err != nil || result.RequeueAfter < busyRetry || result.RequeueAfter > busyRetry*3/2 || calls.Load() != 0 {
		t.Fatalf("busy cleanup: %+v %v", result, err)
	}
	_, err = r.withHostBudget(next, false).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(live)})
	if err != nil || calls.Load() != 1 {
		t.Fatal("cleanup blocked live work", err)
	}
	r.cleanupStalls.failed(deleting.Status.HostID)
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(deleting), deleting); err != nil {
		t.Fatal(err)
	}
	if len(deleting.Finalizers) == 0 {
		t.Fatal("retry suppression dropped a finalizer")
	}
}

func TestEndpointSlicesStayBoundedAndStableDuringRemoval(t *testing.T) {
	svc := newRouteService(testServiceName)
	w := newAvailableWorkload("ready", svc.Name, testHostID)
	r := newRouteReconciler(t, svc, w, newHost("ready", testHostID, testPodIP, routeTestPort))
	runReconcile(t, r, svc.Name)
	desired := getSlice(t, r, svc.Name).DeepCopy()
	desired.ResourceVersion = ""
	desired.Endpoints = nil
	for i := range 251 {
		desired.Endpoints = append(desired.Endpoints, discoveryv1.Endpoint{Addresses: []string{fmt.Sprintf("10.0.%d.%d", i/250, i%250+1)}})
	}
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatal(err)
	}
	list := &discoveryv1.EndpointSliceList{}
	if err := r.List(t.Context(), list); err != nil {
		t.Fatal(err)
	}
	if len(list.Items) != 3 {
		t.Fatalf("got %d slices for 251 endpoints", len(list.Items))
	}
	versions := make(map[string]string)
	for _, slice := range list.Items {
		if len(slice.Endpoints) > 100 {
			t.Fatal("unbounded slice")
		}
		versions[slice.Name] = slice.ResourceVersion
	}
	desired.Endpoints = desired.Endpoints[1:]
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatal(err)
	}
	if err := r.List(t.Context(), list); err != nil {
		t.Fatal(err)
	}
	changed := 0
	for _, slice := range list.Items {
		if slice.ResourceVersion != versions[slice.Name] {
			changed++
		}
	}
	if changed != 1 {
		t.Fatalf("single endpoint removal rewrote %d slices", changed)
	}
}

func TestStatusObservationExpiresAndCannotPrecedePlacement(t *testing.T) {
	s := &statusBatches{entries: map[string]statusObservation{"host/id": {status: &runtimev2.WorkloadStatus{WorkloadId: "id"}, at: time.Now().Add(-time.Hour)}}}
	if s.cached("host", "id", time.Time{}, time.Now().Add(-2*time.Hour)) != nil {
		t.Fatal("expired status reused")
	}
}

func TestPlacementRejectsStaleLiveObjectAfterDeletion(t *testing.T) {
	w := placedWorkload()
	w.Status.WorkloadID = ""
	w.Spec.HostID = w.Status.HostID
	w.Finalizers = []string{workloadFinalizerName}
	current := w.DeepCopy()
	now := metav1.Now()
	current.DeletionTimestamp = &now
	c := newWorkloadClient(t, current)
	bus := &mockBus{}
	r := &WorkloadReconciler{Client: c, APIReader: c, Bus: bus}
	if err := r.reconcilePlacement(t.Context(), w); !condition.IsStatusUnknown(err) {
		t.Fatalf("stale placement returned %v", err)
	}
	if len(bus.requests) != 0 {
		t.Fatal("started a workload after deletion")
	}
}

func TestCleanupCannotOverlapLiveReconciliationOfSameWorkload(t *testing.T) {
	w := placedWorkload()
	w.Finalizers = []string{workloadFinalizerName}
	now := metav1.Now()
	w.DeletionTimestamp = &now
	r := &WorkloadReconciler{Client: newWorkloadClient(t, w), HostConcurrency: 2}
	key := client.ObjectKeyFromObject(w)
	release, ok := r.objectSlots.acquire(key.String(), 1)
	if !ok {
		t.Fatal("cannot occupy object slot")
	}
	defer release()
	next := reconcile.Func(func(context.Context, reconcile.Request) (reconcile.Result, error) {
		t.Fatal("cleanup overlapped an in-flight start")
		return reconcile.Result{}, nil
	})
	result, err := r.withHostBudget(next, true).Reconcile(t.Context(), reconcile.Request{NamespacedName: key})
	if err != nil || result.RequeueAfter < busyRetry || result.RequeueAfter > busyRetry*3/2 {
		t.Fatalf("cleanup did not yield: %+v %v", result, err)
	}
}

func TestReplicaDeleteCostIsBoundedAcrossFleetSizes(t *testing.T) {
	for _, size := range []int{100, 1000, 10000} {
		t.Run(fmt.Sprint(size), func(t *testing.T) {
			synctest.Test(t, func(t *testing.T) {
				rs := cleanupReplicaSet()
				rs.Spec.Replicas = new(int32(0))
				objects := make([]client.Object, 0, size+1)
				objects = append(objects, rs)
				for i := range size {
					objects = append(objects, cleanupWorkload(rs, fmt.Sprintf("replica-%05d", i)))
				}
				var calls, active, peak atomic.Int32
				r := newCleanupReconciler(t, interceptor.Funcs{
					Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
						calls.Add(1)
						n := active.Add(1)
						defer active.Add(-1)
						for previous := peak.Load(); n > previous; previous = peak.Load() {
							if peak.CompareAndSwap(previous, n) {
								break
							}
						}
						time.Sleep(time.Millisecond)
						return c.Delete(ctx, obj, opts...)
					},
				}, objects...)
				r.BatchSize, r.Concurrency, r.APIReader = 64, 4, r.Client
				// Every fleet here needs more than one batch, so ScaleDown stays open.
				if err := r.reconcileScaleDown(t.Context(), rs); !condition.IsStatusUnknown(err) {
					t.Fatal(err)
				}
				if calls.Load() != 64 || peak.Load() != 4 || active.Load() != 0 {
					t.Fatalf("fleet=%d deletes=%d peak=%d active=%d", size, calls.Load(), peak.Load(), active.Load())
				}
			})
		})
	}
}

func TestCleanupSuppressionRetainsReadyHostsOnly(t *testing.T) {
	for _, status := range []corev1.ConditionStatus{condition.ConditionTrue, condition.ConditionUnknown, condition.ConditionFalse} {
		t.Run(string(status), func(t *testing.T) {
			w := placedWorkload()
			w.Finalizers = []string{workloadFinalizerName}
			now := metav1.Now()
			w.DeletionTimestamp = &now
			host := &runtimev1alpha1.Host{ObjectMeta: metav1.ObjectMeta{Name: "suppressed-host", Namespace: testNamespace}, HostID: w.Status.HostID}
			host.Status.SetConditions(condition.Condition{Type: condition.TypeReady, Status: status})
			bus := &mockBus{}
			r := &WorkloadReconciler{Client: newWorkloadClient(t, w, host), Bus: bus, OperatorNamespace: testNamespace, HostConcurrency: 1}
			r.cleanupStalls.failed(w.Status.HostID)
			next := reconcile.Func(func(ctx context.Context, _ reconcile.Request) (reconcile.Result, error) {
				return reconcile.Result{}, r.finalize(ctx, w)
			})
			result, err := r.withHostBudget(next, true).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(w)})
			if err != nil || len(bus.requests) != 0 {
				t.Fatalf("suppression repeated a stop: %v, calls=%d", err, len(bus.requests))
			}
			// Ready hosts keep their finalizers and back off. Unknown and failed
			// hosts are dropped as finalize's stop would after one timeout, so a
			// stalled host does not drain at HostConcurrency per stall window.
			if status == condition.ConditionTrue && result.RequeueAfter != workloadEvictionStopTimeout {
				t.Fatal("ready host must retain cleanup responsibility")
			}
			if status != condition.ConditionTrue && result.RequeueAfter != 0 {
				t.Fatal("unavailable host must not repeat the timeout for each workload")
			}
		})
	}
}

func TestBusyBackoffSpreadsWaitersByHostBacklog(t *testing.T) {
	var b busyBackoff
	upper := func(rounds int) time.Duration {
		d := min(busyRetry*time.Duration(rounds), busyRetryMax)
		return d + d/2
	}
	// Two slots: the first two waiters retry within a round; later waiters,
	// new or old, get the delay of the backlog ahead of them.
	for i := range 6 {
		got := b.retry("h", fmt.Sprintf("cleanup/w%d", i), reconcile.Request{}, 2).RequeueAfter
		if rounds := i/2 + 1; got < busyRetry*time.Duration(rounds) || got > upper(rounds) {
			t.Fatalf("waiter %d: %v", i, got)
		}
	}
	// A repeat miss does not count twice, and another host has its own backlog.
	if got := b.retry("h", "cleanup/w0", reconcile.Request{}, 2).RequeueAfter; got > upper(3) {
		t.Fatalf("repeat waiter: %v", got)
	}
	if got := b.retry("other", "cleanup/x", reconcile.Request{}, 2).RequeueAfter; got > upper(1) {
		t.Fatalf("other host inherited backlog: %v", got)
	}
	for i := range 6 {
		b.done(fmt.Sprintf("cleanup/w%d", i))
	}
	b.done("cleanup/x")
	if len(b.waiting) != 0 || len(b.hostOf) != 0 {
		t.Fatalf("waiters leaked: %v %v", b.waiting, b.hostOf)
	}
}

func TestLiveQueueDropsDeletingWorkloadWithoutWaitingOnItsSlot(t *testing.T) {
	w := placedWorkload()
	w.Finalizers = []string{workloadFinalizerName}
	now := metav1.Now()
	w.DeletionTimestamp = &now
	r := &WorkloadReconciler{Client: newWorkloadClient(t, w), HostConcurrency: 2}
	key := client.ObjectKeyFromObject(w)
	release, ok := r.objectSlots.acquire(key.String(), 1)
	if !ok {
		t.Fatal("cannot occupy object slot")
	}
	defer release()
	next := reconcile.Func(func(context.Context, reconcile.Request) (reconcile.Result, error) {
		t.Fatal("live queue reconciled a deleting workload")
		return reconcile.Result{}, nil
	})
	result, err := r.withHostBudget(next, false).Reconcile(t.Context(), reconcile.Request{NamespacedName: key})
	if err != nil || result.RequeueAfter != 0 {
		t.Fatalf("live queue retried a deleting workload: %+v %v", result, err)
	}
}

func TestStartsDoNotTakeLiveSlots(t *testing.T) {
	placed := placedWorkload()
	starting := placedWorkload()
	starting.Name = "budget-starting"
	starting.Status.WorkloadID = ""
	starting.Status.Conditions = nil
	r := &WorkloadReconciler{Client: newWorkloadClient(t, placed, starting), HostConcurrency: 1}
	host := placed.Status.HostID
	releaseStart, ok := r.startSlots.acquire(host, 1)
	if !ok {
		t.Fatal("cannot occupy start slot")
	}
	var calls atomic.Int32
	next := reconcile.Func(func(context.Context, reconcile.Request) (reconcile.Result, error) {
		calls.Add(1)
		return reconcile.Result{}, nil
	})
	if _, err := r.withHostBudget(next, false).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(placed)}); err != nil || calls.Load() != 1 {
		t.Fatal("a start in flight blocked a placed workload's live pass", err)
	}
	result, err := r.withHostBudget(next, false).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(starting)})
	if err != nil || result.RequeueAfter == 0 || calls.Load() != 1 {
		t.Fatalf("a second start ran past the host's start slots: %+v %v", result, err)
	}
	releaseStart()
	releaseLive, ok := r.liveSlots.acquire(host, 1)
	if !ok {
		t.Fatal("cannot occupy live slot")
	}
	defer releaseLive()
	if _, err := r.withHostBudget(next, false).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(starting)}); err != nil || calls.Load() != 2 {
		t.Fatal("busy live slots blocked a start", err)
	}
}

func TestOnlyAnUnansweredStopMarksTheHostStalled(t *testing.T) {
	if !stopShowsHostStalled(t.Context(), errors.New("nats: timeout")) {
		t.Fatal("an unanswered stop did not count against the host")
	}
	if !stopShowsHostStalled(t.Context(), nil) {
		t.Fatal("a stop given up on an unavailable host did not count against it")
	}
	if stopShowsHostStalled(t.Context(), hostLookupError{errors.New("cache not synced")}) {
		t.Fatal("a failed Host lookup counted against the host")
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if stopShowsHostStalled(ctx, context.Canceled) {
		t.Fatal("a cancelled reconcile counted against the host")
	}
}

func TestReleasedSlotWakesTheLongestWaiter(t *testing.T) {
	waiting := placedWorkload()
	waiting.Name = "wake-waiting"
	running := placedWorkload()
	running.Name = "wake-running"
	r := &WorkloadReconciler{Client: newWorkloadClient(t, waiting, running), HostConcurrency: 1,
		liveWake: make(chan event.GenericEvent, 1)}
	next := reconcile.Func(func(context.Context, reconcile.Request) (reconcile.Result, error) {
		return reconcile.Result{}, nil
	})
	release, ok := r.liveSlots.acquire(waiting.Status.HostID, 1)
	if !ok {
		t.Fatal("cannot occupy live slot")
	}
	if result, err := r.withHostBudget(next, false).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(waiting)}); err != nil || result.RequeueAfter == 0 {
		t.Fatalf("waiter was not held back: %+v %v", result, err)
	}
	release()
	if _, err := r.withHostBudget(next, false).Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(running)}); err != nil {
		t.Fatal(err)
	}
	select {
	case e := <-r.liveWake:
		if e.Object.GetName() != waiting.Name {
			t.Fatalf("woke %s", e.Object.GetName())
		}
	default:
		t.Fatal("a released slot did not wake its waiter")
	}
}

func TestNewEndpointSliceSkipsAForeignHolderOfItsName(t *testing.T) {
	svc := newRouteService(testServiceName)
	w := newAvailableWorkload("ready", svc.Name, testHostID)
	r := newRouteReconciler(t, svc, w, newHost("ready", testHostID, testPodIP, routeTestPort))
	runReconcile(t, r, svc.Name)
	desired := getSlice(t, r, svc.Name).DeepCopy()
	desired.ResourceVersion = ""
	desired.Endpoints = nil
	for i := range 101 {
		desired.Endpoints = append(desired.Endpoints, discoveryv1.Endpoint{Addresses: []string{fmt.Sprintf("10.0.0.%d", i+1)}})
	}
	// An unrelated slice, outside this controller's labels, holds "-1".
	foreign := &discoveryv1.EndpointSlice{AddressType: discoveryv1.AddressTypeIPv4}
	foreign.Name = endpointSliceName(fmt.Sprintf("%s-%d", desired.Name, 1))
	foreign.Namespace = desired.Namespace
	if err := r.Create(t.Context(), foreign); err != nil {
		t.Fatal(err)
	}
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatalf("a foreign slice blocked routing: %v", err)
	}
	list := &discoveryv1.EndpointSliceList{}
	if err := r.List(t.Context(), list, client.MatchingLabels{routeManagerLabel: "true"}); err != nil {
		t.Fatal(err)
	}
	total := 0
	for _, slice := range list.Items {
		if slice.Name == foreign.Name {
			t.Fatal("took over the foreign slice")
		}
		total += len(slice.Endpoints)
	}
	if len(list.Items) != 2 || total != 101 {
		t.Fatalf("got %d slices with %d endpoints", len(list.Items), total)
	}
}

// A pass planned against a cache that had not seen a slice it created can
// publish an endpoint in two slices. The slice's arrival wakes the Service,
// and that pass keeps the endpoint in one slice only.
func TestEndpointSliceDuplicateFromStaleCacheIsRemoved(t *testing.T) {
	svc := newRouteService(testServiceName)
	w := newAvailableWorkload("ready", svc.Name, testHostID)
	r := newRouteReconciler(t, svc, w, newHost("ready", testHostID, testPodIP, routeTestPort))
	runReconcile(t, r, svc.Name)
	desired := getSlice(t, r, svc.Name).DeepCopy()
	desired.ResourceVersion = ""
	desired.Endpoints = nil
	for i := range 101 {
		desired.Endpoints = append(desired.Endpoints, discoveryv1.Endpoint{Addresses: []string{fmt.Sprintf("10.0.0.%d", i+1)}})
	}
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatal(err)
	}
	// Reproduce the stale pass's result: one endpoint withdrawn, and the
	// overflow slice's endpoint also packed into the base slice.
	base, overflow := &discoveryv1.EndpointSlice{}, &discoveryv1.EndpointSlice{}
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(desired), base); err != nil {
		t.Fatal(err)
	}
	overflowKey := client.ObjectKey{Namespace: desired.Namespace, Name: endpointSliceName(fmt.Sprintf("%s-%d", desired.Name, 1))}
	if err := r.Get(t.Context(), overflowKey, overflow); err != nil {
		t.Fatal(err)
	}
	if len(overflow.Endpoints) != 1 {
		t.Fatalf("overflow slice has %d endpoints", len(overflow.Endpoints))
	}
	base.Endpoints = append(base.Endpoints[1:], overflow.Endpoints[0])
	if err := r.Update(t.Context(), base); err != nil {
		t.Fatal(err)
	}
	desired.Endpoints = desired.Endpoints[1:]
	if got := managedSliceServiceRequest(t.Context(), overflow); len(got) != 1 || got[0].Name != svc.Name {
		t.Fatalf("a managed slice event mapped to %v", got)
	}
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatal(err)
	}
	list := &discoveryv1.EndpointSliceList{}
	if err := r.List(t.Context(), list); err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	for _, slice := range list.Items {
		for _, ep := range slice.Endpoints {
			if seen[ep.Addresses[0]] {
				t.Fatalf("endpoint %s still published twice", ep.Addresses[0])
			}
			seen[ep.Addresses[0]] = true
		}
	}
	if len(seen) != 100 || len(list.Items) != 1 {
		t.Fatalf("got %d slices with %d endpoints, want 1 with 100", len(list.Items), len(seen))
	}
}

func TestManagedSliceServiceRequestIgnoresOtherControllers(t *testing.T) {
	svc := newRouteService(testServiceName)
	svc.UID = "svc-uid"
	owned := &discoveryv1.EndpointSlice{}
	owned.Namespace = svc.Namespace
	owned.OwnerReferences = []metav1.OwnerReference{*metav1.NewControllerRef(svc, corev1.SchemeGroupVersion.WithKind("Service"))}
	// An owned slice without the route-manager label is still ours.
	if got := managedSliceServiceRequest(t.Context(), owned); len(got) != 1 || got[0].Name != svc.Name {
		t.Fatalf("unlabeled owned slice mapped to %v", got)
	}
	builtin := owned.DeepCopy()
	builtin.Labels = map[string]string{discoveryv1.LabelManagedBy: "endpointslice-controller.k8s.io"}
	if got := managedSliceServiceRequest(t.Context(), builtin); len(got) != 0 {
		t.Fatalf("built-in slice mapped to %v", got)
	}
	foreign := &discoveryv1.EndpointSlice{}
	foreign.Namespace = svc.Namespace
	foreign.Labels = map[string]string{discoveryv1.LabelServiceName: svc.Name}
	if got := managedSliceServiceRequest(t.Context(), foreign); len(got) != 0 {
		t.Fatalf("unmanaged slice mapped to %v", got)
	}
}

func TestEndpointSliceThatLostItsLabelIsAdopted(t *testing.T) {
	svc := newRouteService(testServiceName)
	w := newAvailableWorkload("ready", svc.Name, testHostID)
	r := newRouteReconciler(t, svc, w, newHost("ready", testHostID, testPodIP, routeTestPort))
	runReconcile(t, r, svc.Name)
	desired := getSlice(t, r, svc.Name).DeepCopy()
	desired.ResourceVersion = ""
	desired.Endpoints = nil
	for i := range 251 {
		desired.Endpoints = append(desired.Endpoints, discoveryv1.Endpoint{Addresses: []string{fmt.Sprintf("10.0.%d.%d", i/250, i%250+1)}})
	}
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatal(err)
	}
	// Strip the route-manager label from an overflow slice.
	overflow := &discoveryv1.EndpointSlice{}
	key := client.ObjectKey{Namespace: desired.Namespace, Name: endpointSliceName(fmt.Sprintf("%s-%d", desired.Name, 1))}
	if err := r.Get(t.Context(), key, overflow); err != nil {
		t.Fatal(err)
	}
	delete(overflow.Labels, routeManagerLabel)
	if err := r.Update(t.Context(), overflow); err != nil {
		t.Fatal(err)
	}
	// A slice the built-in controller manages for this Service is left alone.
	builtin := &discoveryv1.EndpointSlice{AddressType: discoveryv1.AddressTypeIPv4}
	builtin.Name, builtin.Namespace = "builtin", desired.Namespace
	builtin.Labels = map[string]string{
		discoveryv1.LabelServiceName: svc.Name,
		discoveryv1.LabelManagedBy:   "endpointslice-controller.k8s.io",
	}
	builtin.OwnerReferences = desired.OwnerReferences
	if err := r.Create(t.Context(), builtin); err != nil {
		t.Fatal(err)
	}
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatalf("an unlabeled owned slice blocked routing: %v", err)
	}
	list := &discoveryv1.EndpointSliceList{}
	if err := r.List(t.Context(), list, client.MatchingLabels{routeManagerLabel: "true"}); err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	for _, slice := range list.Items {
		for _, ep := range slice.Endpoints {
			if seen[ep.Addresses[0]] {
				t.Fatalf("endpoint %s published twice", ep.Addresses[0])
			}
			seen[ep.Addresses[0]] = true
		}
	}
	if len(list.Items) != 3 || len(seen) != 251 {
		t.Fatalf("got %d labeled slices with %d endpoints", len(list.Items), len(seen))
	}
	desired.Endpoints = nil
	if err := r.syncSlices(t.Context(), desired); err != nil {
		t.Fatal(err)
	}
	if err := r.List(t.Context(), list); err != nil {
		t.Fatal(err)
	}
	if len(list.Items) != 1 || list.Items[0].Name != builtin.Name {
		t.Fatalf("expected only the built-in slice to remain, got %d slices", len(list.Items))
	}
}
