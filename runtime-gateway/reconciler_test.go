package main

import (
	"context"
	"slices"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// httpInterface builds a wasi:http host interface advertising the given handler
// interface, optionally with a routing `host` config.
func httpInterface(iface, host string) runtimev1alpha1.HostInterface {
	hi := runtimev1alpha1.HostInterface{
		Namespace:  "wasi",
		Package:    "http",
		Interfaces: []string{iface},
	}
	if host != "" {
		hi.Config = map[string]string{"host": host}
	}
	return hi
}

func TestWorkloadHostname(t *testing.T) {
	tests := []struct {
		name   string
		ifaces []runtimev1alpha1.HostInterface
		want   string
	}{
		{
			// p2 handler entrypoint.
			name:   "incoming-handler is routed",
			ifaces: []runtimev1alpha1.HostInterface{httpInterface("incoming-handler", "a.localhost.direct")},
			want:   "a.localhost.direct",
		},
		{
			// p3 handler entrypoint — the case the gateway previously dropped, so
			// a p3 handler workload reached Ready but every request to it 503'd.
			name:   "handler is routed",
			ifaces: []runtimev1alpha1.HostInterface{httpInterface("handler", "b.localhost.direct")},
			want:   "b.localhost.direct",
		},
		{
			name:   "handler without a host config yields no route",
			ifaces: []runtimev1alpha1.HostInterface{httpInterface("handler", "")},
			want:   "",
		},
		{
			name: "non-http interface is ignored",
			ifaces: []runtimev1alpha1.HostInterface{{
				Namespace:   "acme",
				Package:     "kv",
				Interfaces:  []string{"store"},
				ConfigLayer: runtimev1alpha1.ConfigLayer{Config: map[string]string{"host": "ignored"}},
			}},
			want: "",
		},
		{
			name:   "no interfaces yields no route",
			ifaces: nil,
			want:   "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			w := &runtimev1alpha1.Workload{}
			w.Spec.HostInterfaces = tt.ifaces
			if got := workloadHostname(w); got != tt.want {
				t.Errorf("workloadHostname() = %q, want %q", got, tt.want)
			}
		})
	}
}

// The finalizer the operator sets on the objects the gateway also watches.
const operatorWorkloadFinalizer = "runtime.wasmcloud.dev/workload-finalizer"

// recordingRegistry records what the reconcilers register, keyed the way the
// tracker keys it.
type recordingRegistry struct {
	workloads map[types.NamespacedName]workloadRoute
	hosts     map[types.NamespacedName]string
}

func newRecordingRegistry() *recordingRegistry {
	return &recordingRegistry{
		workloads: make(map[types.NamespacedName]workloadRoute),
		hosts:     make(map[types.NamespacedName]string),
	}
}

func (r *recordingRegistry) RegisterWorkload(_ context.Context, key types.NamespacedName, hostID string, workloadID string, hostname string) error {
	r.workloads[key] = workloadRoute{hostID: hostID, workloadID: workloadID, hostname: hostname}
	return nil
}

func (r *recordingRegistry) DeregisterWorkload(_ context.Context, key types.NamespacedName) error {
	delete(r.workloads, key)
	return nil
}

func (r *recordingRegistry) RegisterHost(_ context.Context, key types.NamespacedName, hostID string, hostname string, port int) error {
	r.hosts[key] = hostID
	return nil
}

func (r *recordingRegistry) DeregisterHost(_ context.Context, key types.NamespacedName) error {
	delete(r.hosts, key)
	return nil
}

func testScheme(t *testing.T) *runtime.Scheme {
	t.Helper()
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("AddToScheme() = %v", err)
	}
	return s
}

// availableWorkload builds a placed, ready Workload with an HTTP route.
func availableWorkload(name string) *runtimev1alpha1.Workload {
	w := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Namespace: "ns", Name: name},
	}
	w.Spec.HostInterfaces = []runtimev1alpha1.HostInterface{httpInterface("incoming-handler", "a.localhost.direct")}
	w.Status.HostID = "host-id"
	w.Status.WorkloadID = "workload-id"
	w.Status.SetConditions(
		condition.Condition{Type: runtimev1alpha1.WorkloadConditionPlacement, Status: condition.ConditionTrue},
		condition.Condition{Type: condition.TypeReady, Status: condition.ConditionTrue},
	)
	return w
}

func testRoute() workloadRoute {
	return workloadRoute{hostID: "host-id", workloadID: "workload-id", hostname: "a.localhost.direct"}
}

// A Workload can be fully gone by the time its deletion reaches the reconciler,
// so the route has to come out of the registry on the request key alone.
func TestWorkloadReconcilerDeregistersMissingWorkload(t *testing.T) {
	key := types.NamespacedName{Namespace: "ns", Name: "workload-a"}
	registry := newRecordingRegistry()
	registry.workloads[key] = testRoute()

	r := &WorkloadReconciler{
		Client:   fake.NewClientBuilder().WithScheme(testScheme(t)).Build(),
		Registry: registry,
	}
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		t.Fatalf("Reconcile() = %v", err)
	}

	if _, ok := registry.workloads[key]; ok {
		t.Error("route for a deleted Workload is still registered")
	}
}

// The gateway's routing tables live only in the process that built them, so a
// gateway finalizer outlives everything that could release it: a Workload
// carrying one can never finish deleting once the gateway is gone.
func TestWorkloadReconcilerDropsGatewayFinalizer(t *testing.T) {
	workload := availableWorkload("workload-a")
	workload.Finalizers = []string{operatorWorkloadFinalizer, gatewayWorkloadFinalizerName}
	key := client.ObjectKeyFromObject(workload)

	c := fake.NewClientBuilder().WithScheme(testScheme(t)).WithObjects(workload).Build()
	r := &WorkloadReconciler{Client: c, Registry: newRecordingRegistry()}
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		t.Fatalf("Reconcile() = %v", err)
	}

	got := &runtimev1alpha1.Workload{}
	if err := c.Get(t.Context(), key, got); err != nil {
		t.Fatalf("Get() = %v", err)
	}
	if slices.Contains(got.Finalizers, gatewayWorkloadFinalizerName) {
		t.Error("gateway finalizer is still on the Workload")
	}
	if !slices.Contains(got.Finalizers, operatorWorkloadFinalizer) {
		t.Error("operator finalizer was dropped")
	}
}

func TestWorkloadReconcilerRegistersAvailableWorkload(t *testing.T) {
	workload := availableWorkload("workload-a")
	key := client.ObjectKeyFromObject(workload)
	registry := newRecordingRegistry()

	c := fake.NewClientBuilder().WithScheme(testScheme(t)).WithObjects(workload).Build()
	r := &WorkloadReconciler{Client: c, Registry: registry}
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		t.Fatalf("Reconcile() = %v", err)
	}

	if got, want := registry.workloads[key], testRoute(); got != want {
		t.Errorf("registered route = %+v, want %+v", got, want)
	}

	got := &runtimev1alpha1.Workload{}
	if err := c.Get(t.Context(), key, got); err != nil {
		t.Fatalf("Get() = %v", err)
	}
	if len(got.Finalizers) != 0 {
		t.Errorf("Finalizers = %v, want none", got.Finalizers)
	}
}

// A Workload under deletion still exists but must stop taking traffic.
func TestWorkloadReconcilerDeregistersDeletingWorkload(t *testing.T) {
	workload := availableWorkload("workload-a")
	workload.Finalizers = []string{operatorWorkloadFinalizer}
	deletedAt := metav1.Now()
	workload.DeletionTimestamp = &deletedAt
	key := client.ObjectKeyFromObject(workload)

	registry := newRecordingRegistry()
	registry.workloads[key] = testRoute()

	c := fake.NewClientBuilder().WithScheme(testScheme(t)).WithObjects(workload).Build()
	r := &WorkloadReconciler{Client: c, Registry: registry}
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		t.Fatalf("Reconcile() = %v", err)
	}

	if _, ok := registry.workloads[key]; ok {
		t.Error("route for a deleting Workload is still registered")
	}
}

func TestHostReconcilerDeregistersMissingHost(t *testing.T) {
	key := types.NamespacedName{Namespace: "ns", Name: "host-a"}
	registry := newRecordingRegistry()
	registry.hosts[key] = "host-id"

	r := &HostReconciler{
		Client:   fake.NewClientBuilder().WithScheme(testScheme(t)).Build(),
		Registry: registry,
	}
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		t.Fatalf("Reconcile() = %v", err)
	}

	if _, ok := registry.hosts[key]; ok {
		t.Error("a deleted Host is still registered")
	}
}

func TestHostReconcilerDropsGatewayFinalizer(t *testing.T) {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{
			Namespace:  "ns",
			Name:       "host-a",
			Finalizers: []string{gatewayHostFinalizerName},
		},
		HostID:   "host-id",
		Hostname: "10.0.0.1",
		HTTPPort: 8080,
	}
	key := client.ObjectKeyFromObject(host)

	c := fake.NewClientBuilder().WithScheme(testScheme(t)).WithObjects(host).Build()
	r := &HostReconciler{Client: c, Registry: newRecordingRegistry()}
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
		t.Fatalf("Reconcile() = %v", err)
	}

	got := &runtimev1alpha1.Host{}
	if err := c.Get(t.Context(), key, got); err != nil {
		t.Fatalf("Get() = %v", err)
	}
	if len(got.Finalizers) != 0 {
		t.Errorf("Finalizers = %v, want none", got.Finalizers)
	}
}
