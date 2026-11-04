package runtime

import (
	"errors"
	"testing"
	"testing/synctest"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestDaemonSetFailureGraceBoundaryAndDeleteRetry(t *testing.T) {
	for _, hostRecovered := range []bool{false, true} {
		name := "failure starts grace"
		if hostRecovered {
			name = "host recovery restarts grace"
		}
		t.Run(name, func(t *testing.T) {
			synctest.Test(t, func(t *testing.T) {
				ds := testDaemonSet(1)
				now := time.Now()
				failedAt := now
				host := defaultGroupHost(dsHostA)
				host.Status.Conditions[0].LastTransitionTime = metav1.NewTime(now.Add(-time.Hour))
				if hostRecovered {
					failedAt = now.Add(-time.Hour)
					host.Status.Conditions[0].LastTransitionTime = metav1.NewTime(now)
				}
				r := newDaemonSetReconciler(t, ds, host, defaultGroupHost(dsHostB))
				schedule(t, r, ds)
				for hostID, workloads := range workloadsByHost(t, r) {
					workload := &workloads[0]
					failed := condition.ErrorCondition(runtimev1alpha1.WorkloadConditionSync, "Reconcile", errors.New("start failed"))
					failed.LastTransitionTime = metav1.NewTime(failedAt)
					workload.Status.SetConditions(failed)
					if hostID == dsHostB {
						workload.Status.SetConditions(condition.Available())
					}
					if err := r.Update(t.Context(), workload); err != nil {
						t.Fatal(err)
					}
				}
				before := workloadsByHost(t, r)
				key := client.ObjectKeyFromObject(&before[dsHostA][0])
				remove := daemonSetWrite{daemonSetDeleteOperation, key}
				want := errors.New("expired instance delete refused")
				writes := recordDaemonSetWrites(r, remove, want)
				for _, advance := range []time.Duration{0, workloadReplicaSetReplicaGracePeriod - time.Nanosecond} {
					time.Sleep(advance)
					schedule(t, r, ds)
					for _, workloads := range before {
						assertDaemonSetWorkloadsUnchanged(t, r, workloads)
					}
					assertDaemonSetWrites(t, *writes)
				}
				time.Sleep(time.Nanosecond)
				if err := r.reconcileSchedule(t.Context(), ds); !errors.Is(err, want) {
					t.Fatalf("reconcile at grace deadline = %v, want delete error", err)
				}
				assertDaemonSetWrites(t, *writes, remove)
				assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostA])
				assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostB])
				schedule(t, r, ds)
				assertDaemonSetWorkloadAbsent(t, r, key)
				assertDaemonSetWorkloadsUnchanged(t, r, before[dsHostB])
				assertDaemonSetWrites(t, *writes, remove, remove)
				if ds.Status.Replicas.Current != 1 || ds.Status.Replicas.Ready != 1 {
					t.Fatalf("expired instance still counted: %+v", ds.Status.Replicas)
				}
				schedule(t, r, ds)
				assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1, dsHostB: 1})
				replacement := workloadsByHost(t, r)[dsHostA][0]
				if len(replacement.Status.Conditions) != 0 || replacement.Spec.HostID != dsHostA {
					t.Fatalf("replacement retained failed status or moved hosts: %+v", replacement)
				}
				assertDaemonSetWrites(t, *writes, remove, remove, daemonSetWrite{daemonSetCreateOperation, key})
				after := workloadsByHost(t, r)
				schedule(t, r, ds)
				assertDaemonSetWrites(t, *writes, remove, remove, daemonSetWrite{daemonSetCreateOperation, key})
				for _, workloads := range after {
					assertDaemonSetWorkloadsUnchanged(t, r, workloads)
				}
			})
		})
	}
}
