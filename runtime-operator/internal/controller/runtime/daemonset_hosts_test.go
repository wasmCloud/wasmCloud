package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"
	"sigs.k8s.io/controller-runtime/pkg/event"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// isolatedDaemonSetReconciler is newDaemonSetReconciler with sharing disabled
// and the Host Environment index the isolated host list needs.
func isolatedDaemonSetReconciler(t *testing.T, funcs interceptor.Funcs, objs ...client.Object) *WorkloadDaemonSetReconciler {
	t.Helper()
	s := newScaleTestScheme(t)
	c := daemonSetClientBuilder(t, s, objs...).
		WithIndex(&runtimev1alpha1.Host{}, hostEnvironmentIndex, workloadHostIndexes(t)[hostEnvironmentIndex]).
		WithInterceptorFuncs(funcs).
		Build()
	return &WorkloadDaemonSetReconciler{Client: c, APIReader: c, Scheme: s, OperatorNamespace: testNamespace}
}

func hostInEnvironment(hostID, environment string) *runtimev1alpha1.Host {
	host := defaultGroupHost(hostID)
	host.Environment = environment
	return host
}

// With sharing disabled, a DaemonSet runs only on its own tenant's hosts.
func TestDaemonSetSchedule_IsolatedKeepsToOwnEnvironment(t *testing.T) {
	ds := testDaemonSet(1)
	r := isolatedDaemonSetReconciler(t, interceptor.Funcs{}, ds,
		hostInEnvironment(dsHostA, testDaemonSetNamespace), hostInEnvironment(dsHostB, otherTenant))
	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
}

// A template that targets another tenant's Environment is refused outright,
// rather than running on no hosts as if none matched.
func TestDaemonSetSchedule_RefusesCrossEnvironmentTarget(t *testing.T) {
	ds := testDaemonSet(1)
	ds.Spec.Template.Spec.Environment = otherTenant
	r := isolatedDaemonSetReconciler(t, interceptor.Funcs{}, ds, hostInEnvironment(dsHostA, otherTenant))
	if err := r.reconcileSchedule(context.Background(), ds); !errors.Is(err, errCrossEnvironmentDenied) {
		t.Fatalf("reconcileSchedule returned %v, want a cross-environment denial", err)
	}
	assertPerHost(t, workloadsByHost(t, r), map[string]int{})
}

// A Host that has not reported its ID yet cannot be pinned to, so it gets no
// instances until it does.
func TestDaemonSetSchedule_UnregisteredHostGetsNothing(t *testing.T) {
	ds := testDaemonSet(1)
	unregistered := defaultGroupHost(dsHostB)
	unregistered.HostID = ""
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), unregistered)
	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
}

// A Host being deleted is not retained like one that is merely not Ready: its
// instances go with it.
func TestDaemonSetSchedule_DeletingHostLosesInstances(t *testing.T) {
	ds := testDaemonSet(1)
	hostB := defaultGroupHost(dsHostB)
	hostB.Finalizers = []string{hostFinalizerName}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), hostB)
	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1, dsHostB: 1})

	if err := r.Delete(context.Background(), hostB); err != nil {
		t.Fatalf("delete host: %v", err)
	}
	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
}

// An instance being deleted is no longer one of the host's replicas, but its
// name is still in use: the replacement takes the next one.
func TestDaemonSetSchedule_TerminatingInstanceIsReplaced(t *testing.T) {
	ds := testDaemonSet(1)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))
	schedule(t, r, ds)
	terminating := workloadsByHost(t, r)[dsHostA][0]
	terminating.Finalizers = []string{"test.wasmcloud.dev/hold"}
	if err := r.Update(context.Background(), &terminating); err != nil {
		t.Fatalf("hold workload: %v", err)
	}
	if err := r.Delete(context.Background(), &terminating); err != nil {
		t.Fatalf("delete workload: %v", err)
	}

	schedule(t, r, ds)
	var live []string
	for _, w := range workloadsByHost(t, r)[dsHostA] {
		if w.DeletionTimestamp == nil {
			live = append(live, w.Name)
		}
	}
	if len(live) != 1 || live[0] == terminating.Name {
		t.Fatalf("live instances %v, want one replacement for %s", live, terminating.Name)
	}
}

// An instance on a host that left must be removed; when the delete fails the
// pass fails, rather than reporting the instance gone.
func TestDaemonSetSchedule_DepartedHostDeleteFailure(t *testing.T) {
	ds := testDaemonSet(1)
	hostB := defaultGroupHost(dsHostB)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), hostB)
	schedule(t, r, ds)
	if err := r.Delete(context.Background(), hostB); err != nil {
		t.Fatalf("delete host: %v", err)
	}

	// Only the departed host's instance fails to delete, so the error can
	// come from nowhere else.
	r.Client = interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{
		Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
			if w, ok := obj.(*runtimev1alpha1.Workload); ok && w.Spec.HostID == dsHostB {
				return errAPIUnavailable
			}
			return c.Delete(ctx, obj, opts...)
		},
	})
	if err := r.reconcileSchedule(context.Background(), ds); !errors.Is(err, errAPIUnavailable) {
		t.Fatalf("reconcileSchedule returned %v, want the delete error", err)
	}
}

// A name collision is settled by reading the live object; when that read
// fails the slot is not taken over blind.
func TestDaemonSetSchedule_CollisionReadFailure(t *testing.T) {
	ds := testDaemonSet(1)
	template := ds.Spec.Template.DeepCopy()
	hash := template.Hash()
	name := daemonSetWorkloadName(testDaemonSetName, dsHostA, hash, 0)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphanedWorkload(name, dsHostA, hash))
	r.APIReader = failingReader{}

	err := r.syncHost(context.Background(), ds, template, hash, dsHostA, &daemonSetHost{}, 1, map[string]bool{})
	if !errors.Is(err, errUncachedRead) {
		t.Fatalf("syncHost returned %v, want the read error", err)
	}
}

func TestDaemonSetsForHost_UnusableEvents(t *testing.T) {
	r := newDaemonSetReconciler(t, testDaemonSet(1))
	if got := r.daemonSetsForHost(context.Background(), &runtimev1alpha1.Workload{}); got != nil {
		t.Errorf("a non-Host object enqueued %v", got)
	}
	r.Client = interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{List: failHostList})
	if got := r.daemonSetsForHost(context.Background(), defaultGroupHost(dsHostA)); got != nil {
		t.Errorf("a failed list enqueued %v", got)
	}
}

// The predicate only filters Host updates; anything else it is handed passes,
// so a misrouted event is never silently dropped.
func TestHostPlacementChangedPassesOtherObjects(t *testing.T) {
	if !hostPlacementChanged.Update(event.UpdateEvent{
		ObjectOld: &runtimev1alpha1.Workload{}, ObjectNew: &runtimev1alpha1.Workload{},
	}) {
		t.Fatal("a non-Host update was filtered out")
	}
	host := defaultGroupHost(dsHostA)
	heartbeat := host.DeepCopy()
	heartbeat.Status.LastSeen = metav1.NewTime(time.Now())
	if hostPlacementChanged.Update(event.UpdateEvent{ObjectOld: host, ObjectNew: heartbeat}) {
		t.Fatal("a heartbeat that changes nothing about placement passed")
	}
}
