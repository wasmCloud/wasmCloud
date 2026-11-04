package runtime

import (
	"context"
	"reflect"
	"testing"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	daemonSetCreateOperation = "create"
	daemonSetDeleteOperation = "delete"
	daemonSetUpdatedImage    = "echo:v2"
)

type daemonSetWrite struct {
	operation string
	key       client.ObjectKey
}

func recordDaemonSetWrites(r *WorkloadDaemonSetReconciler, fail daemonSetWrite, cause error) *[]daemonSetWrite {
	var writes []daemonSetWrite
	failed := false
	attempt := func(operation string, obj client.Object) error {
		write := daemonSetWrite{operation, client.ObjectKeyFromObject(obj)}
		writes = append(writes, write)
		if !failed && write == fail {
			failed = true
			return cause
		}
		return nil
	}
	r.Client = interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{
		Create: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.CreateOption) error {
			if err := attempt(daemonSetCreateOperation, obj); err != nil {
				return err
			}
			return c.Create(ctx, obj, opts...)
		},
		Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
			if err := attempt(daemonSetDeleteOperation, obj); err != nil {
				return err
			}
			return c.Delete(ctx, obj, opts...)
		},
	})
	return &writes
}

func assertDaemonSetWrites(t *testing.T, got []daemonSetWrite, want ...daemonSetWrite) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("write attempts = %v, want %v", got, want)
	}
}

func assertDaemonSetWorkloadsUnchanged(t *testing.T, r *WorkloadDaemonSetReconciler, before []runtimev1alpha1.Workload) {
	t.Helper()
	for _, want := range before {
		got := &runtimev1alpha1.Workload{}
		if err := r.Get(t.Context(), client.ObjectKeyFromObject(&want), got); err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(got, &want) {
			t.Fatalf("workload %s changed: before=%+v after=%+v", want.Name, want, got)
		}
	}
}

func assertDaemonSetWorkloadAbsent(t *testing.T, r *WorkloadDaemonSetReconciler, key client.ObjectKey) {
	t.Helper()
	if err := r.Get(t.Context(), key, &runtimev1alpha1.Workload{}); !apierrors.IsNotFound(err) {
		t.Fatalf("Get(%s) = %v, want NotFound", key, err)
	}
}

func TestDaemonSetScaleDownKeepsReadyInstances(t *testing.T) {
	ds := testDaemonSet(3)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB))
	schedule(t, r, ds)
	before := workloadsByHost(t, r)
	unready := map[string]int{dsHostA: 1, dsHostB: 0}
	for hostID, workloads := range before {
		for i := range workloads {
			if i == unready[hostID] {
				continue
			}
			workloads[i].Status.SetConditions(condition.Available())
			if err := r.Update(t.Context(), &workloads[i]); err != nil {
				t.Fatal(err)
			}
		}
	}
	before = workloadsByHost(t, r)
	writes := recordDaemonSetWrites(r, daemonSetWrite{}, nil)
	ds.Spec.ReplicasPerHost = new(int32(2))
	for range 2 {
		schedule(t, r, ds)
		assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 2, dsHostB: 2})
		for hostID, workloads := range before {
			for i, workload := range workloads {
				if i == unready[hostID] {
					assertDaemonSetWorkloadAbsent(t, r, client.ObjectKeyFromObject(&workload))
				} else {
					assertDaemonSetWorkloadsUnchanged(t, r, []runtimev1alpha1.Workload{workload})
				}
			}
		}
		assertDaemonSetWrites(t, *writes,
			daemonSetWrite{daemonSetDeleteOperation, client.ObjectKeyFromObject(&before[dsHostA][1])},
			daemonSetWrite{daemonSetDeleteOperation, client.ObjectKeyFromObject(&before[dsHostB][0])},
		)
		if ds.Status.Replicas.Current != 4 || ds.Status.Replicas.Ready != 4 || ds.Status.ReadyHosts != 2 {
			t.Fatalf("scale-down status = %+v", ds.Status)
		}
	}
}

type daemonSetRolloutState struct {
	current  int32
	outdated int32
	ready    int32
}

func assertDaemonSetRollout(t *testing.T, r *WorkloadDaemonSetReconciler, ds *runtimev1alpha1.WorkloadDaemonSet,
	want map[string]daemonSetRolloutState,
) {
	t.Helper()
	byHost := workloadsByHost(t, r)
	if len(byHost) != len(want) {
		t.Fatalf("hosts = %v, want %v", byHost, want)
	}
	var current, ready, readyHosts, updatedHosts int32
	attention := make(map[string]runtimev1alpha1.WorkloadDaemonSetHostStatus)
	for hostID, expected := range want {
		var got daemonSetRolloutState
		for _, workload := range byHost[hostID] {
			if workload.Annotations[workloadDaemonSetGenerationAnnotation] == ds.Spec.Template.Hash() {
				got.current++
				if workload.Status.IsAvailable() {
					got.ready++
				}
			} else {
				got.outdated++
			}
		}
		if got != expected {
			t.Fatalf("host %s: rollout state = %+v, want %+v", hostID, got, expected)
		}
		current += got.current
		ready += got.ready
		if got.ready >= *ds.Spec.ReplicasPerHost {
			readyHosts++
		}
		if got.outdated == 0 {
			updatedHosts++
		}
		if got.ready < *ds.Spec.ReplicasPerHost || got.outdated != 0 {
			attention[hostID] = runtimev1alpha1.WorkloadDaemonSetHostStatus{
				HostID: hostID, Current: got.current, Ready: got.ready, Outdated: got.outdated,
			}
		}
	}
	status := ds.Status
	if status.DesiredHosts != int32(len(want)) || status.ReadyHosts != readyHosts || status.UpdatedHosts != updatedHosts ||
		status.Replicas.Expected != int32(len(want))**ds.Spec.ReplicasPerHost || status.Replicas.Current != current ||
		status.Replicas.Ready != ready || status.Replicas.Unavailable != current-ready {
		t.Fatalf("rollout status = %+v, replicas = %+v", status, status.Replicas)
	}
	gotAttention := make(map[string]runtimev1alpha1.WorkloadDaemonSetHostStatus)
	for _, host := range status.Hosts {
		gotAttention[host.HostID] = host
	}
	if !reflect.DeepEqual(gotAttention, attention) {
		t.Fatalf("hosts needing attention = %v, want %v", gotAttention, attention)
	}
}

func TestDaemonSetRolloutRespectsTwoHostSlots(t *testing.T) {
	const hostD = "ds-host-d"
	ds := testDaemonSet(2)
	ds.Spec.MaxConcurrentHosts = new(int32(2))
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB),
		defaultGroupHost(dsHostC), defaultGroupHost(hostD))
	schedule(t, r, ds)
	markAllAvailable(t, r)
	old := workloadsByHost(t, r)
	ds.Spec.Template.Spec.Components[0].Image = daemonSetUpdatedImage
	markReady := func(hostID string, count int) {
		t.Helper()
		marked := 0
		for _, workload := range workloadsByHost(t, r)[hostID] {
			if marked == count || workload.Annotations[workloadDaemonSetGenerationAnnotation] != ds.Spec.Template.Hash() {
				continue
			}
			workload.Status.SetConditions(condition.Available())
			if err := r.Update(t.Context(), &workload); err != nil {
				t.Fatal(err)
			}
			marked++
		}
		if marked != count {
			t.Fatalf("marked %d current instances on %s ready, want %d", marked, hostID, count)
		}
	}
	schedule(t, r, ds)
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {2, 2, 0}, dsHostB: {2, 2, 0}, dsHostC: {0, 2, 0}, hostD: {0, 2, 0},
	})
	markReady(dsHostA, 1)
	schedule(t, r, ds)
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {2, 2, 1}, dsHostB: {2, 2, 0}, dsHostC: {0, 2, 0}, hostD: {0, 2, 0},
	})
	for _, workloads := range old {
		assertDaemonSetWorkloadsUnchanged(t, r, workloads)
	}
	markReady(dsHostA, 2)
	schedule(t, r, ds)
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {2, 0, 2}, dsHostB: {2, 2, 0}, dsHostC: {2, 2, 0}, hostD: {0, 2, 0},
	})
	markReady(dsHostB, 2)
	markReady(dsHostC, 2)
	schedule(t, r, ds)
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {2, 0, 2}, dsHostB: {2, 0, 2}, dsHostC: {2, 0, 2}, hostD: {2, 2, 0},
	})
	markReady(hostD, 1)
	schedule(t, r, ds)
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {2, 0, 2}, dsHostB: {2, 0, 2}, dsHostC: {2, 0, 2}, hostD: {2, 2, 1},
	})
	assertDaemonSetWorkloadsUnchanged(t, r, old[hostD])
	markReady(hostD, 2)
	schedule(t, r, ds)
	assertDaemonSetRollout(t, r, ds, map[string]daemonSetRolloutState{
		dsHostA: {2, 0, 2}, dsHostB: {2, 0, 2}, dsHostC: {2, 0, 2}, hostD: {2, 0, 2},
	})
	before := workloadsByHost(t, r)
	writes := recordDaemonSetWrites(r, daemonSetWrite{}, nil)
	schedule(t, r, ds)
	assertDaemonSetWrites(t, *writes)
	for hostID, workloads := range before {
		assertDaemonSetWorkloadsUnchanged(t, r, workloads)
		for _, workload := range old[hostID] {
			assertDaemonSetWorkloadAbsent(t, r, client.ObjectKeyFromObject(&workload))
		}
	}
}
