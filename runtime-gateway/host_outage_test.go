package main

import (
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// hostOutage drives a HostReconciler backed by a real HostTracker through
// Host readiness changes.
type hostOutage struct {
	t       *testing.T
	client  client.Client
	tracker *HostTracker
	hosts   map[string]*runtimev1alpha1.Host
}

func newHostOutage(t *testing.T, ready condition.Condition, names ...string) *hostOutage {
	t.Helper()
	o := &hostOutage{t: t, tracker: newHostTracker(testFallback{}), hosts: map[string]*runtimev1alpha1.Host{}}
	builder := fake.NewClientBuilder().WithScheme(testScheme(t))
	for _, name := range names {
		host := &runtimev1alpha1.Host{
			ObjectMeta: metav1.ObjectMeta{Namespace: "ns", Name: name},
			HostID:     name,
			Hostname:   name,
			HTTPPort:   8080,
		}
		host.Status.SetConditions(ready)
		o.hosts[name] = host
		builder = builder.WithObjects(host).WithStatusSubresource(host)
	}
	o.client = builder.Build()
	for _, name := range names {
		o.reconcile(name)
		key := types.NamespacedName{Namespace: "ns", Name: name}
		if err := o.tracker.RegisterWorkload(t.Context(), key, name, name, "shared.example"); err != nil {
			t.Fatal(err)
		}
	}
	return o
}

func (o *hostOutage) setReady(name string, ready condition.Condition) {
	o.t.Helper()
	host := o.hosts[name]
	host.Status.SetConditions(ready)
	if err := o.client.Status().Update(o.t.Context(), host); err != nil {
		o.t.Fatal(err)
	}
	o.reconcile(name)
}

func (o *hostOutage) delete(name string) {
	o.t.Helper()
	if err := o.client.Delete(o.t.Context(), o.hosts[name]); err != nil {
		o.t.Fatal(err)
	}
	o.reconcile(name)
}

func (o *hostOutage) reconcile(name string) {
	o.t.Helper()
	r := &HostReconciler{Client: o.client, Registry: o.tracker}
	key := types.NamespacedName{Namespace: "ns", Name: name}
	if _, err := r.Reconcile(o.t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		o.t.Fatal(err)
	}
}

func notReadyConditions() map[string]condition.Condition {
	return map[string]condition.Condition{
		"false":   condition.Unavailable(),
		"unknown": {Type: condition.TypeReady, Status: condition.ConditionUnknown},
	}
}

// A Host that is not Ready for a while gets its workload routes back when it is
// Ready again; no Workload event comes to register them again.
func TestHostOutageKeepsWorkloadRoutes(t *testing.T) {
	for name, notReady := range notReadyConditions() {
		t.Run(name, func(t *testing.T) {
			o := newHostOutage(t, condition.ReadyCondition(condition.TypeReady), "a")
			o.setReady("a", notReady)
			if got := resolve(t, o.tracker, "shared.example"); got.Hostname != fallbackEndpoint {
				t.Fatalf("not-ready host takes traffic: %+v", got)
			}
			o.setReady("a", condition.ReadyCondition(condition.TypeReady))
			want := LookupResult{Scheme: "http", Hostname: "a:8080", WorkloadID: "a"}
			if got := resolve(t, o.tracker, "shared.example"); got != want {
				t.Fatalf("route after the host is Ready again = %+v, want %+v", got, want)
			}
		})
	}
}

// While one host of a shared hostname is not Ready, every request goes to the
// other host.
func TestHostOutageRoutesSharedHostnameToReadyHost(t *testing.T) {
	for name, notReady := range notReadyConditions() {
		t.Run(name, func(t *testing.T) {
			o := newHostOutage(t, condition.ReadyCondition(condition.TypeReady), "a", "b")
			o.setReady("a", notReady)
			want := LookupResult{Scheme: "http", Hostname: "b:8080", WorkloadID: "b"}
			for range 50 {
				if got := resolve(t, o.tracker, "shared.example"); got != want {
					t.Fatalf("route while host a is not Ready = %+v, want %+v", got, want)
				}
			}
		})
	}
}

// On gateway startup during an outage, Workloads can register routes on a Host
// the gateway has never seen Ready; deleting that Host drops them.
func TestHostOutageDeletingNeverReadyHostDropsItsRoutes(t *testing.T) {
	for name, notReady := range notReadyConditions() {
		t.Run(name, func(t *testing.T) {
			o := newHostOutage(t, notReady, "a")
			o.delete("a")
			if len(o.tracker.workloadKeys) != 0 {
				t.Errorf("workloadKeys = %v, want the deleted host's routes dropped", o.tracker.workloadKeys)
			}
			// A later host under the same ID must not serve the deleted host's routes.
			if err := o.tracker.RegisterHost(t.Context(), types.NamespacedName{Namespace: "ns", Name: "a-new"}, "a", "a-new", 8080); err != nil {
				t.Fatal(err)
			}
			if got := resolve(t, o.tracker, "shared.example"); got.Hostname != fallbackEndpoint {
				t.Errorf("Resolve() = %+v after the host was deleted, want the fallback", got)
			}
		})
	}
}
