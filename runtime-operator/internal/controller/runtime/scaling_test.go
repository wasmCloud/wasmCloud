package runtime

import (
	"context"
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
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

func TestReplicaBatchesYieldAndPreserveTerminatingVictims(t *testing.T) {
	rs := cleanupReplicaSet()
	rs.Spec.Replicas = new(int32(10))
	r := newCleanupReconciler(t, interceptor.Funcs{}, rs)
	r.BatchSize, r.Concurrency, r.APIReader = 3, 2, r.Client
	if err := r.reconcileScaleUp(t.Context(), rs); err != nil {
		t.Fatal(err)
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
	if err != nil || result.RequeueAfter != busyRetry || calls.Load() != 0 {
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
	if s.cached("host", "id", time.Time{}) != nil {
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
	if err != nil || result.RequeueAfter != busyRetry {
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
				if err := r.reconcileScaleDown(t.Context(), rs); err != nil {
					t.Fatal(err)
				}
				if calls.Load() != 64 || peak.Load() != 4 || active.Load() != 0 {
					t.Fatalf("fleet=%d deletes=%d peak=%d active=%d", size, calls.Load(), peak.Load(), active.Load())
				}
			})
		})
	}
}

func TestCleanupSuppressionRetainsUnknownAndReadyHosts(t *testing.T) {
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
			if status != condition.ConditionFalse && result.RequeueAfter != workloadEvictionStopTimeout {
				t.Fatal("possibly running host must retain cleanup responsibility")
			}
			if status == condition.ConditionFalse && result.RequeueAfter != 0 {
				t.Fatal("known failed host must not repeat the timeout for each workload")
			}
		})
	}
}
