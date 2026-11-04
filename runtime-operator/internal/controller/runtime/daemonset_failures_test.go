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
			replicas := int32(1)
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
			target := remove(old[dsHostA][0])
			wantFailed := []daemonSetWrite{target}
			wantRetried := []daemonSetWrite{target, target}
			absent := []client.ObjectKey{target.key}
			protected := make(map[string][]runtimev1alpha1.Workload)
			deletedBeforeError := client.ObjectKey{}
			switch phase {
			case scaleDown:
				ds.Spec.ReplicasPerHost = new(int32(1))
				target = remove(old[dsHostA][1])
				first := remove(old[dsHostA][0])
				wantFailed = []daemonSetWrite{first, target}
				wantRetried = []daemonSetWrite{first, target, target, remove(old[dsHostB][0]), remove(old[dsHostB][1])}
				absent = []client.ObjectKey{first.key, target.key, wantRetried[3].key, wantRetried[4].key}
				deletedBeforeError = first.key
				protected[dsHostA] = old[dsHostA][2:]
				protected[dsHostB] = old[dsHostB][2:]
			case retireOld:
				ds.Spec.MaxConcurrentHosts = new(int32(2))
				ds.Spec.Template.Spec.Components[0].Image = daemonSetUpdatedImage
				schedule(t, r, ds)
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
					protected[dsHostA] = []runtimev1alpha1.Workload{workload}
				}
				protected[dsHostB] = workloadsByHost(t, r)[dsHostB]
			case recreate:
				ds.Spec.DeployPolicy = runtimev1alpha1.WorkloadDeployPolicyRecreate
				ds.Spec.Template.Spec.Components[0].Image = daemonSetUpdatedImage
				create := func(hostID string) daemonSetWrite {
					return daemonSetWrite{daemonSetCreateOperation, client.ObjectKey{
						Namespace: ds.Namespace, Name: daemonSetWorkloadName(ds.Name, hostID, ds.Spec.Template.Hash(), 0),
					}}
				}
				wantRetried = []daemonSetWrite{target, target, create(dsHostA), remove(old[dsHostB][0]), create(dsHostB)}
				absent = []client.ObjectKey{target.key, client.ObjectKeyFromObject(&old[dsHostB][0])}
			}
			before := workloadsByHost(t, r)
			want := errors.New("delete refused")
			writes := recordDaemonSetWrites(r, target, want)
			if err := r.reconcileSchedule(t.Context(), ds); !errors.Is(err, want) {
				t.Fatalf("reconcileSchedule = %v, want delete error", err)
			}
			assertDaemonSetWrites(t, *writes, wantFailed...)
			for _, workloads := range before {
				for _, workload := range workloads {
					if key := client.ObjectKeyFromObject(&workload); key == deletedBeforeError {
						assertDaemonSetWorkloadAbsent(t, r, key)
					} else {
						assertDaemonSetWorkloadsUnchanged(t, r, []runtimev1alpha1.Workload{workload})
					}
				}
			}
			schedule(t, r, ds)
			wantCounts := map[string]int{dsHostA: 1, dsHostB: 1}
			if phase == retireOld {
				wantCounts[dsHostB] = 2
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
