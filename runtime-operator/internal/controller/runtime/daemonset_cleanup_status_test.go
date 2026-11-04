package runtime

import (
	"errors"
	"testing"
	"testing/synctest"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestDaemonSetReconcilePersistsPartialDeleteStatus(t *testing.T) {
	ds := testDaemonSet(3)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB))
	schedule(t, r, ds)
	markAllAvailable(t, r)
	before := workloadsByHost(t, r)
	key := client.ObjectKeyFromObject(ds)
	if err := r.Get(t.Context(), key, ds); err != nil {
		t.Fatal(err)
	}
	ds.Spec.ReplicasPerHost = new(int32(1))
	if err := r.Update(t.Context(), ds); err != nil {
		t.Fatal(err)
	}
	first := daemonSetWrite{daemonSetDeleteOperation, client.ObjectKeyFromObject(&before[dsHostA][0])}
	failed := daemonSetWrite{daemonSetDeleteOperation, client.ObjectKeyFromObject(&before[dsHostA][1])}
	want := errors.New("second scale-down delete refused")
	writes := recordDaemonSetWrites(r, failed, want)
	reconciler := condition.NewConditionedReconciler(r.Client, r.Scheme,
		&runtimev1alpha1.WorkloadDaemonSet{}, workloadDaemonSetReconcileInterval)
	reconciler.SetCondition(runtimev1alpha1.WorkloadDaemonSetConditionSchedule, r.reconcileSchedule)
	result, err := reconciler.Reconcile(t.Context(), reconcile.Request{NamespacedName: key})
	if err != nil || result.RequeueAfter <= 0 {
		t.Fatalf("Reconcile = %+v, %v; want a retry", result, err)
	}
	assertDaemonSetWrites(t, *writes, first, failed)
	if err := r.Get(t.Context(), key, ds); err != nil {
		t.Fatal(err)
	}
	scheduleCondition := ds.Status.GetCondition(runtimev1alpha1.WorkloadDaemonSetConditionSchedule)
	if scheduleCondition.Status != condition.ConditionFalse || scheduleCondition.Message != want.Error() {
		t.Fatalf("persisted Schedule condition = %+v", scheduleCondition)
	}
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {2, 0, 2}, dsHostB: {3, 0, 3},
	})
	if _, err := reconciler.Reconcile(t.Context(), reconcile.Request{NamespacedName: key}); err != nil {
		t.Fatal(err)
	}
	if err := r.Get(t.Context(), key, ds); err != nil {
		t.Fatal(err)
	}
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {1, 0, 1}, dsHostB: {1, 0, 1},
	})
	if !ds.Status.AllTrue(runtimev1alpha1.WorkloadDaemonSetConditionSchedule) {
		t.Fatalf("retry did not clear the Schedule failure: %+v", ds.Status.Conditions)
	}
}

func TestDaemonSetFailedCleanupReportsPartialDeletes(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ds := testDaemonSet(4)
		host := defaultGroupHost(dsHostA)
		host.Status.Conditions[0].LastTransitionTime = metav1.NewTime(time.Now().Add(-time.Hour))
		r := newDaemonSetReconciler(t, ds, host, defaultGroupHost(dsHostB))
		schedule(t, r, ds)
		markAllAvailable(t, r)
		for i, workload := range workloadsByHost(t, r)[dsHostA] {
			if i != 0 && i != 2 {
				continue
			}
			failed := condition.ErrorCondition(runtimev1alpha1.WorkloadConditionSync, "Reconcile", errors.New("start failed"))
			failed.LastTransitionTime = metav1.NewTime(time.Now().Add(-time.Hour))
			workload.Status.Conditions = []condition.Condition{failed}
			if err := r.Update(t.Context(), &workload); err != nil {
				t.Fatal(err)
			}
		}
		before := workloadsByHost(t, r)
		firstKey := client.ObjectKeyFromObject(&before[dsHostA][0])
		failedKey := client.ObjectKeyFromObject(&before[dsHostA][2])
		first := daemonSetWrite{daemonSetDeleteOperation, firstKey}
		failed := daemonSetWrite{daemonSetDeleteOperation, failedKey}
		want := errors.New("second failed-instance delete refused")
		writes := recordDaemonSetWrites(r, failed, want)
		if err := r.reconcileSchedule(t.Context(), ds); !errors.Is(err, want) {
			t.Fatalf("reconcileSchedule = %v, want delete error", err)
		}
		assertDaemonSetWrites(t, *writes, first, failed)
		assertDaemonSetWorkloadAbsent(t, r, firstKey)
		assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostA][1:])
		assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostB])
		assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
			dsHostA: {3, 0, 2}, dsHostB: {4, 0, 4},
		})
		createFirst := daemonSetWrite{daemonSetCreateOperation, firstKey}
		schedule(t, r, ds)
		assertDaemonSetWrites(t, *writes, first, failed, createFirst, failed)
		assertDaemonSetWorkloadAbsent(t, r, failedKey)
		assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostA][1:2])
		assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostA][3:])
		assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostB])
		assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
			dsHostA: {3, 0, 2}, dsHostB: {4, 0, 4},
		})
		createSecond := daemonSetWrite{daemonSetCreateOperation, failedKey}
		schedule(t, r, ds)
		assertDaemonSetWrites(t, *writes, first, failed, createFirst, failed, createSecond)
		assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
			dsHostA: {4, 0, 2}, dsHostB: {4, 0, 4},
		})
		after := workloadsByHost(t, r)
		schedule(t, r, ds)
		assertDaemonSetWrites(t, *writes, first, failed, createFirst, failed, createSecond)
		for _, workloads := range after {
			assertDaemonSetWorkloadsUnchanged(t, r, workloads)
		}
	})
}
