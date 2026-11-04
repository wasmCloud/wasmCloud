package runtime

import (
	"errors"
	"testing"

	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestDaemonSetCreateFailureRetriesOnlyMissingInstances(t *testing.T) {
	ds := testDaemonSet(1)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB), defaultGroupHost(dsHostC))
	create := func(hostID string) daemonSetWrite {
		return daemonSetWrite{daemonSetCreateOperation, client.ObjectKey{
			Namespace: ds.Namespace, Name: daemonSetWorkloadName(ds.Name, hostID, ds.Spec.Template.Hash(), 0),
		}}
	}
	want := errors.New("create refused")
	writes := recordDaemonSetWrites(r, create(dsHostB), want)
	if err := r.reconcileSchedule(t.Context(), ds); !errors.Is(err, want) {
		t.Fatalf("reconcileSchedule = %v, want create error", err)
	}
	assertDaemonSetWrites(t, *writes, create(dsHostA), create(dsHostB))
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
	assertDaemonSetWorkloadAbsent(t, r, create(dsHostB).key)
	assertDaemonSetWorkloadAbsent(t, r, create(dsHostC).key)
	if ds.Status.Replicas.Current != 1 || ds.Status.Replicas.Expected != 3 {
		t.Fatalf("status after partial create = %+v", ds.Status.Replicas)
	}
	markAllAvailable(t, r)
	first := workloadsByHost(t, r)[dsHostA]
	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1, dsHostB: 1, dsHostC: 1})
	assertDaemonSetWorkloadsUnchanged(t, r, first)
	assertDaemonSetWrites(t, *writes, create(dsHostA), create(dsHostB), create(dsHostB), create(dsHostC))
	before := workloadsByHost(t, r)
	schedule(t, r, ds)
	assertDaemonSetWrites(t, *writes, create(dsHostA), create(dsHostB), create(dsHostB), create(dsHostC))
	for _, workloads := range before {
		assertDaemonSetWorkloadsUnchanged(t, r, workloads)
	}
}

func TestDaemonSetDeleteFailuresPreserveWorkloadsAndRetry(t *testing.T) {
	const (
		scaleDown = "scale down"
		retireOld = "retire old template"
		recreate  = "recreate"
	)
	for _, phase := range []string{scaleDown, retireOld, recreate} {
		t.Run(phase, func(t *testing.T) {
			replicas := int32(2)
			if phase == scaleDown {
				replicas = 3
			}
			ds := testDaemonSet(replicas)
			r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB))
			schedule(t, r, ds)
			markAllAvailable(t, r)
			old := workloadsByHost(t, r)
			remove := func(workload runtimev1alpha1.Workload) daemonSetWrite {
				return daemonSetWrite{daemonSetDeleteOperation, client.ObjectKeyFromObject(&workload)}
			}
			first := remove(old[dsHostA][0])
			target := remove(old[dsHostA][1])
			wantFailed := []daemonSetWrite{first, target}
			wantRetried := []daemonSetWrite{first, target, target}
			absent := []client.ObjectKey{first.key, target.key}
			protected := make(map[string][]runtimev1alpha1.Workload)
			var failedState map[string]daemonSetRolloutState
			switch phase {
			case scaleDown:
				ds.Spec.ReplicasPerHost = new(int32(1))
				wantRetried = []daemonSetWrite{first, target, target, remove(old[dsHostB][0]), remove(old[dsHostB][1])}
				absent = []client.ObjectKey{first.key, target.key, wantRetried[3].key, wantRetried[4].key}
				protected[dsHostA] = old[dsHostA][2:]
				protected[dsHostB] = old[dsHostB][2:]
				failedState = map[string]daemonSetRolloutState{dsHostA: {2, 0, 2}, dsHostB: {3, 0, 3}}
			case retireOld:
				ds.Spec.MaxConcurrentHosts = new(int32(2))
				ds.Spec.Template.Spec.Components[0].Image = daemonSetUpdatedImage
				schedule(t, r, ds)
				protected[dsHostA] = make([]runtimev1alpha1.Workload, 0, replicas)
				for _, workload := range workloadsByHost(t, r)[dsHostA] {
					if workload.Annotations[workloadDaemonSetGenerationAnnotation] != ds.Spec.Template.Hash() {
						continue
					}
					workload.Status.SetConditions(condition.Available())
					if err := r.Update(t.Context(), &workload); err != nil {
						t.Fatal(err)
					}
					if err := r.Get(t.Context(), client.ObjectKeyFromObject(&workload), &workload); err != nil {
						t.Fatal(err)
					}
					protected[dsHostA] = append(protected[dsHostA], workload)
				}
				protected[dsHostB] = workloadsByHost(t, r)[dsHostB]
				failedState = map[string]daemonSetRolloutState{dsHostA: {2, 1, 2}, dsHostB: {2, 2, 0}}
			case recreate:
				ds.Spec.DeployPolicy = runtimev1alpha1.WorkloadDeployPolicyRecreate
				ds.Spec.Template.Spec.Components[0].Image = daemonSetUpdatedImage
				create := func(hostID string, slot int) daemonSetWrite {
					return daemonSetWrite{daemonSetCreateOperation, client.ObjectKey{
						Namespace: ds.Namespace, Name: daemonSetWorkloadName(ds.Name, hostID, ds.Spec.Template.Hash(), slot),
					}}
				}
				wantRetried = []daemonSetWrite{
					first, target, target, create(dsHostA, 0), create(dsHostA, 1),
					remove(old[dsHostB][0]), remove(old[dsHostB][1]), create(dsHostB, 0), create(dsHostB, 1),
				}
				absent = []client.ObjectKey{
					first.key, target.key, client.ObjectKeyFromObject(&old[dsHostB][0]), client.ObjectKeyFromObject(&old[dsHostB][1]),
				}
				failedState = map[string]daemonSetRolloutState{dsHostA: {0, 1, 0}, dsHostB: {0, 2, 0}}
			}
			before := workloadsByHost(t, r)
			want := errors.New("delete refused")
			writes := recordDaemonSetWrites(r, target, want)
			if err := r.reconcileSchedule(t.Context(), ds); !errors.Is(err, want) {
				t.Fatalf("reconcileSchedule = %v, want delete error", err)
			}
			assertDaemonSetWrites(t, *writes, wantFailed...)
			assertDaemonSetRollout(t, r, ds, failedState)
			for _, workloads := range before {
				for _, workload := range workloads {
					if key := client.ObjectKeyFromObject(&workload); key == first.key {
						assertDaemonSetWorkloadAbsent(t, r, key)
					} else {
						assertDaemonSetWorkloadsUnchanged(t, r, []runtimev1alpha1.Workload{workload})
					}
				}
			}
			schedule(t, r, ds)
			wantCounts := map[string]int{dsHostA: int(*ds.Spec.ReplicasPerHost), dsHostB: int(*ds.Spec.ReplicasPerHost)}
			if phase == retireOld {
				wantCounts[dsHostB] *= 2
			}
			assertPerHost(t, workloadsByHost(t, r), wantCounts)
			assertDaemonSetWrites(t, *writes, wantRetried...)
			for _, key := range absent {
				assertDaemonSetWorkloadAbsent(t, r, key)
			}
			for _, workloads := range protected {
				assertDaemonSetWorkloadsUnchanged(t, r, workloads)
			}
			after := workloadsByHost(t, r)
			schedule(t, r, ds)
			assertDaemonSetWrites(t, *writes, wantRetried...)
			for _, workloads := range after {
				assertDaemonSetWorkloadsUnchanged(t, r, workloads)
			}
		})
	}
}
