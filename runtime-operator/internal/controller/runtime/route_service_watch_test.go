package runtime

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	discoveryv1 "k8s.io/api/discovery/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/wait"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/config"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	"sigs.k8s.io/controller-runtime/pkg/metrics/server"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// A Service created after its Workloads are Ready must get an EndpointSlice
// without waiting for some Workload to change.
func TestWorkloadRouteBuildsSliceForServiceCreatedLater(t *testing.T) {
	if os.Getenv("KUBEBUILDER_ASSETS") == "" {
		t.Skip("KUBEBUILDER_ASSETS not set; run via make test for envtest")
	}
	env := &envtest.Environment{
		CRDDirectoryPaths:     []string{filepath.Join("..", "..", "..", "config", "crd", "bases")},
		ErrorIfCRDPathMissing: true,
	}
	cfg, err := env.Start()
	if err != nil {
		t.Fatalf("start envtest: %v", err)
	}
	t.Cleanup(func() {
		if err := env.Stop(); err != nil {
			t.Errorf("stop envtest: %v", err)
		}
	})

	scheme := newRouteTestScheme(t)
	apiClient, err := client.New(cfg, client.Options{Scheme: scheme})
	if err != nil {
		t.Fatal(err)
	}
	mgr, err := ctrl.NewManager(cfg, ctrl.Options{
		Scheme:     scheme,
		Metrics:    server.Options{BindAddress: "0"},
		Controller: config.Controller{SkipNameValidation: new(true)},
	})
	if err != nil {
		t.Fatal(err)
	}
	// Production registers the Host index in WorkloadReconciler.SetupWithManager.
	if err := mgr.GetFieldIndexer().IndexField(context.Background(), &runtimev1alpha1.Host{}, hostIDIndex,
		func(obj client.Object) []string {
			if host, ok := obj.(*runtimev1alpha1.Host); ok && host.HostID != "" {
				return []string{host.HostID}
			}
			return nil
		}); err != nil {
		t.Fatal(err)
	}
	const namespace = "route-late-service"
	reconciler := &WorkloadRouteReconciler{Client: mgr.GetClient(), Scheme: scheme, OperatorNamespace: namespace}
	if err := reconciler.SetupWithManager(mgr); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	startHostRecoveryManager(t, ctx, cancel, mgr)

	createTestNamespace(t, ctx, apiClient, namespace)
	host := newHost("host-a", testHostID, testPodIP, 9191)
	host.Namespace = namespace
	if err := apiClient.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Namespace: namespace, Name: "wl-1"},
		Spec: runtimev1alpha1.WorkloadSpec{Kubernetes: &runtimev1alpha1.KubernetesSpec{
			Service: &runtimev1alpha1.KubernetesServiceRef{Name: testServiceName},
		}},
	}
	if err := apiClient.Create(ctx, workload); err != nil {
		t.Fatal(err)
	}
	workload.Status.HostID = testHostID
	workload.Status.SetConditions(condition.Available())
	if err := apiClient.Status().Update(ctx, workload); err != nil {
		t.Fatal(err)
	}
	// Let the Workload events reach the controller while the Service is absent.
	time.Sleep(3 * time.Second)

	svc := newRouteService(testServiceName)
	svc.Namespace = namespace
	if err := apiClient.Create(ctx, svc); err != nil {
		t.Fatal(err)
	}
	slice := &discoveryv1.EndpointSlice{}
	key := types.NamespacedName{Namespace: namespace, Name: endpointSliceName(testServiceName)}
	waitCtx, waitCancel := context.WithTimeout(ctx, 10*time.Second)
	defer waitCancel()
	if err := wait.PollUntilContextCancel(waitCtx, 100*time.Millisecond, true, func(ctx context.Context) (bool, error) {
		return apiClient.Get(ctx, key, slice) == nil, nil
	}); err != nil {
		t.Fatalf("no EndpointSlice for a Service created after its Workloads were Ready: %v", err)
	}
	assertRouteSlice(t, slice, svc, testPodIP)
}

// A slice still owned by a deleted Service is handed to the Service that
// replaced it, so garbage collection does not remove it later.
func TestWorkloadRouteAdoptsSliceOfRecreatedService(t *testing.T) {
	ctx := context.Background()
	oldSvc := newRouteService(testServiceName)
	oldSvc.UID = "old-service-uid"
	r := newRouteReconciler(t, oldSvc,
		newAvailableWorkload("wl-1", testServiceName, testHostID),
		newHost("host-a", testHostID, testPodIP, 9191))
	runReconcile(t, r, testServiceName)
	assertRouteSlice(t, getSlice(t, r, testServiceName), oldSvc, testPodIP)

	if err := r.Delete(ctx, oldSvc); err != nil {
		t.Fatal(err)
	}
	newSvc := newRouteService(testServiceName)
	newSvc.UID = "new-service-uid"
	if err := r.Create(ctx, newSvc); err != nil {
		t.Fatal(err)
	}
	runReconcile(t, r, testServiceName)
	assertRouteSlice(t, getSlice(t, r, testServiceName), newSvc, testPodIP)
}

// Only Services that a Workload routes to are enqueued.
func TestServiceToRequestSkipsUnroutedServices(t *testing.T) {
	ctx := context.Background()
	r := newRouteReconciler(t, newAvailableWorkload("wl-1", testServiceName, testHostID))
	if got := r.serviceToRequest(ctx, newRouteService(testServiceName)); len(got) != 1 {
		t.Fatalf("routed Service requests = %v, want one", got)
	}
	if got := r.serviceToRequest(ctx, newRouteService("unrelated")); len(got) != 0 {
		t.Fatalf("unrouted Service requests = %v, want none", got)
	}
}
