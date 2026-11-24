package runtime

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	discoveryv1 "k8s.io/api/discovery/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/wait"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/config"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/metrics/server"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// routeTestFinalizer holds an object in deletion; envtest runs no garbage collector.
const routeTestFinalizer = "wasmcloud.dev/route-test-hold"

// startRouteController runs the route controller against a fresh API server
// with one Host in namespace, and returns a client that bypasses its cache.
func startRouteController(t *testing.T, namespace string) (context.Context, client.Client) {
	t.Helper()
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
	reconciler := &WorkloadRouteReconciler{Client: mgr.GetClient(), Scheme: scheme, OperatorNamespace: namespace}
	if err := reconciler.SetupWithManager(mgr); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	startHostRecoveryManager(t, ctx, cancel, mgr)

	createTestNamespace(t, ctx, apiClient, namespace)
	host := newHost("host-a", testHostID, testPodIP, routeTestPort)
	host.Namespace = namespace
	if err := apiClient.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	return ctx, apiClient
}

// createReadyRouteWorkload creates an Available Workload on the test host that
// routes to testServiceName.
func createReadyRouteWorkload(t *testing.T, ctx context.Context, apiClient client.Client, namespace string) {
	t.Helper()
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
}

func createRouteService(t *testing.T, ctx context.Context, apiClient client.Client, namespace string) *corev1.Service {
	t.Helper()
	svc := newRouteService(testServiceName)
	svc.Namespace = namespace
	if err := apiClient.Create(ctx, svc); err != nil {
		t.Fatal(err)
	}
	return svc
}

// waitForRouteSlice waits for svc's slice to exist, be owned by svc and not be deleting.
func waitForRouteSlice(ctx context.Context, apiClient client.Client, svc *corev1.Service) (*discoveryv1.EndpointSlice, error) {
	slice := &discoveryv1.EndpointSlice{}
	key := types.NamespacedName{Namespace: svc.Namespace, Name: endpointSliceName(svc.Name)}
	waitCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	err := wait.PollUntilContextCancel(waitCtx, 100*time.Millisecond, true, func(ctx context.Context) (bool, error) {
		if err := apiClient.Get(ctx, key, slice); err != nil {
			return false, client.IgnoreNotFound(err)
		}
		return slice.DeletionTimestamp.IsZero() && metav1.IsControlledBy(slice, svc), nil
	})
	return slice, err
}

func setFinalizers(t *testing.T, ctx context.Context, apiClient client.Client, obj client.Object, finalizers string) {
	t.Helper()
	patch := []byte(`{"metadata":{"finalizers":` + finalizers + `}}`)
	if err := apiClient.Patch(ctx, obj, client.RawPatch(types.MergePatchType, patch)); err != nil {
		t.Fatal(err)
	}
}

// A Service created after its Workloads are Ready must get an EndpointSlice
// without waiting for some Workload to change.
func TestWorkloadRouteBuildsSliceForServiceCreatedLater(t *testing.T) {
	const namespace = "route-late-service"
	ctx, apiClient := startRouteController(t, namespace)
	createReadyRouteWorkload(t, ctx, apiClient, namespace)
	// Let the Workload events reach the controller while the Service is absent.
	time.Sleep(3 * time.Second)

	svc := createRouteService(t, ctx, apiClient, namespace)
	slice, err := waitForRouteSlice(ctx, apiClient, svc)
	if err != nil {
		t.Fatalf("no EndpointSlice for a Service created after its Workloads were Ready: %v", err)
	}
	assertRouteSlice(t, slice, svc, testPodIP)
}

// A replacement Service must get a slice even when the old Service's slice
// was still deleting when the replacement appeared.
func TestWorkloadRouteRebuildsSliceThatWasDeletingAtReplacement(t *testing.T) {
	const namespace = "route-deleting-slice"
	ctx, apiClient := startRouteController(t, namespace)
	createReadyRouteWorkload(t, ctx, apiClient, namespace)
	oldSvc := createRouteService(t, ctx, apiClient, namespace)
	oldSlice, err := waitForRouteSlice(ctx, apiClient, oldSvc)
	if err != nil {
		t.Fatalf("no EndpointSlice for the old Service: %v", err)
	}

	// A finalizer holds the old slice in deletion while the replacement Service appears.
	setFinalizers(t, ctx, apiClient, oldSlice, `["`+routeTestFinalizer+`"]`)
	if err := apiClient.Delete(ctx, oldSvc); err != nil {
		t.Fatal(err)
	}
	if err := apiClient.Delete(ctx, oldSlice); err != nil {
		t.Fatal(err)
	}
	newSvc := createRouteService(t, ctx, apiClient, namespace)
	// Let the controller see the replacement while the old slice is deleting.
	time.Sleep(2 * time.Second)
	setFinalizers(t, ctx, apiClient, oldSlice, `null`)

	slice, err := waitForRouteSlice(ctx, apiClient, newSvc)
	if err != nil {
		t.Fatalf("no EndpointSlice for the replacement Service after the old slice was removed: %v", err)
	}
	assertRouteSlice(t, slice, newSvc, testPodIP)
}

// A slice still owned by a deleted Service is handed to the Service that
// replaced it, so garbage collection does not remove it later.
func TestWorkloadRouteAdoptsSliceOfRecreatedService(t *testing.T) {
	ctx := context.Background()
	oldSvc := newRouteService(testServiceName)
	oldSvc.UID = "old-service-uid"
	r := newRouteReconciler(t, oldSvc,
		newAvailableWorkload("wl-1", testServiceName, testHostID),
		newHost("host-a", testHostID, testPodIP, routeTestPort))
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

// Adopting a slice that is already deleting would not stop the deletion, so
// the controller leaves it and builds a new one once it is gone.
func TestWorkloadRouteDoesNotAdoptDeletingSlice(t *testing.T) {
	ctx := t.Context()
	oldSvc := newRouteService(testServiceName)
	oldSvc.UID = "old-service-uid"
	r := newRouteReconciler(t, oldSvc,
		newAvailableWorkload("wl-1", testServiceName, testHostID),
		newHost("host-a", testHostID, testPodIP, routeTestPort))
	runReconcile(t, r, testServiceName)
	slice := getSlice(t, r, testServiceName)
	slice.Finalizers = []string{routeTestFinalizer}
	if err := r.Update(ctx, slice); err != nil {
		t.Fatal(err)
	}
	if err := r.Delete(ctx, slice); err != nil {
		t.Fatal(err)
	}
	if err := r.Delete(ctx, oldSvc); err != nil {
		t.Fatal(err)
	}
	newSvc := newRouteService(testServiceName)
	newSvc.UID = "new-service-uid"
	if err := r.Create(ctx, newSvc); err != nil {
		t.Fatal(err)
	}

	runReconcile(t, r, testServiceName)
	deleting := getSlice(t, r, testServiceName)
	if !metav1.IsControlledBy(deleting, oldSvc) {
		t.Fatalf("deleting slice was adopted: owners = %v", deleting.OwnerReferences)
	}
	deleting.Finalizers = nil
	if err := r.Update(ctx, deleting); err != nil {
		t.Fatal(err)
	}
	runReconcile(t, r, testServiceName)
	assertRouteSlice(t, getSlice(t, r, testServiceName), newSvc, testPodIP)
}

// A slice under the route slice's name that this controller does not manage
// is never taken over.
func TestWorkloadRouteDoesNotAdoptUnmanagedSlice(t *testing.T) {
	svc := newRouteService(testServiceName)
	svc.UID = routeServiceUID
	foreign := staleRouteSlice()
	foreign.Labels = map[string]string{
		discoveryv1.LabelServiceName: testServiceName,
		discoveryv1.LabelManagedBy:   "someone-else.example.com",
	}
	r := newRouteReconciler(t, svc, foreign,
		newAvailableWorkload("wl-1", testServiceName, testHostID),
		newHost("host-a", testHostID, testPodIP, routeTestPort))
	before := getSlice(t, r, testServiceName)
	if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: client.ObjectKeyFromObject(svc)}); err == nil {
		t.Fatal("reconcile reported success for a slice it does not manage")
	}
	if after := getSlice(t, r, testServiceName); !reflect.DeepEqual(after, before) {
		t.Fatalf("unmanaged slice was changed: before=%+v after=%+v", before, after)
	}
}

// A deleting slice without the route-manager label, ours or not, makes the reconcile
// fail: the slice watch drops its delete event, so only the backoff requeues the Service.
func TestWorkloadRouteRetriesDeletingSliceWithoutTheLabel(t *testing.T) {
	for _, ours := range []bool{false, true} {
		ctx := t.Context()
		svc := newRouteService(testServiceName)
		svc.UID = routeServiceUID
		r := newRouteReconciler(t, svc,
			newAvailableWorkload("wl-1", testServiceName, testHostID),
			newHost("host-a", testHostID, testPodIP, routeTestPort))
		var slice *discoveryv1.EndpointSlice
		if ours {
			runReconcile(t, r, testServiceName)
			slice = getSlice(t, r, testServiceName)
			delete(slice.Labels, routeManagerLabel)
			slice.Finalizers = []string{routeTestFinalizer}
			if err := r.Update(ctx, slice); err != nil {
				t.Fatal(err)
			}
		} else {
			slice = staleRouteSlice()
			slice.Labels = map[string]string{discoveryv1.LabelServiceName: testServiceName}
			slice.Finalizers = []string{routeTestFinalizer}
			if err := r.Create(ctx, slice); err != nil {
				t.Fatal(err)
			}
		}
		if err := r.Delete(ctx, slice); err != nil {
			t.Fatal(err)
		}
		if _, err := r.Reconcile(ctx, ctrl.Request{NamespacedName: client.ObjectKeyFromObject(svc)}); err == nil {
			t.Fatalf("ours=%v: reconcile waited on a deleting slice whose delete event the watch drops", ours)
		}
	}
}

// With no ready endpoints, a same-named slice this controller does not manage
// is not deleted.
func TestWorkloadRouteKeepsUnmanagedSliceWithNoEndpoints(t *testing.T) {
	svc := newRouteService(testServiceName)
	foreign := staleRouteSlice()
	foreign.Labels = map[string]string{discoveryv1.LabelServiceName: testServiceName}
	r := newRouteReconciler(t, svc, foreign)
	before := getSlice(t, r, testServiceName)
	runReconcile(t, r, testServiceName)
	if after := getSlice(t, r, testServiceName); !reflect.DeepEqual(after, before) {
		t.Fatalf("unmanaged slice was changed: before=%+v after=%+v", before, after)
	}
}

// A Service held in deletion, as by a load balancer finalizer, keeps getting
// its slice updated and removed; only a new slice is not built.
func TestWorkloadRouteKeepsSliceCurrentWhileServiceIsDeleting(t *testing.T) {
	const otherHostID, otherPodIP = "host-def", "10.244.0.43"
	ctx := t.Context()
	svc := newRouteService(testServiceName)
	svc.Finalizers = []string{routeTestFinalizer}
	first := newAvailableWorkload("wl-1", testServiceName, testHostID)
	r := newRouteReconciler(t, svc, first, newHost("host-a", testHostID, testPodIP, routeTestPort))
	runReconcile(t, r, testServiceName)
	if err := r.Delete(ctx, svc); err != nil {
		t.Fatal(err)
	}
	if err := r.Get(ctx, client.ObjectKeyFromObject(svc), svc); err != nil {
		t.Fatal(err)
	}

	second := newAvailableWorkload("wl-2", testServiceName, otherHostID)
	for _, obj := range []client.Object{newHost("host-b", otherHostID, otherPodIP, routeTestPort), second} {
		if err := r.Create(ctx, obj); err != nil {
			t.Fatal(err)
		}
	}
	runReconcile(t, r, testServiceName)
	assertRouteSlice(t, getSlice(t, r, testServiceName), svc, testPodIP, otherPodIP)

	for _, wl := range []client.Object{first, second} {
		if err := r.Delete(ctx, wl); err != nil {
			t.Fatal(err)
		}
	}
	runReconcile(t, r, testServiceName)
	if got := getSlice(t, r, testServiceName); got != nil {
		t.Fatalf("the slice of a deleting Service was not removed with its last endpoint: %v", got.Endpoints)
	}
}

// A Service being deleted gets no new slice: garbage collection would delete
// it again and the slice watch would rebuild it, round after round.
func TestWorkloadRouteBuildsNoSliceForDeletingService(t *testing.T) {
	svc := newRouteService(testServiceName)
	svc.Finalizers = []string{routeTestFinalizer}
	r := newRouteReconciler(t, svc,
		newAvailableWorkload("wl-1", testServiceName, testHostID),
		newHost("host-a", testHostID, testPodIP, routeTestPort))
	if err := r.Delete(t.Context(), svc); err != nil {
		t.Fatal(err)
	}
	runReconcile(t, r, testServiceName)
	if got := getSlice(t, r, testServiceName); got != nil {
		t.Fatalf("deleting Service got an EndpointSlice owned by %v", got.OwnerReferences)
	}
}

// Only the deletion of a slice this controller manages enqueues its Service.
func TestManagedSliceDeletedFilter(t *testing.T) {
	managed := &discoveryv1.EndpointSlice{ObjectMeta: metav1.ObjectMeta{
		Namespace: testNamespace,
		Name:      endpointSliceName(testServiceName),
		Labels:    map[string]string{discoveryv1.LabelServiceName: testServiceName, routeManagerLabel: routeManagedValue},
	}}
	other := managed.DeepCopy()
	other.Labels = map[string]string{
		discoveryv1.LabelServiceName: testServiceName,
		discoveryv1.LabelManagedBy:   "endpointslice-controller.k8s.io",
	}
	for _, tc := range []struct {
		name string
		got  bool
		want bool
	}{
		{"managed slice deleted", managedSliceDeleted.Delete(event.DeleteEvent{Object: managed}), true},
		{"other slice deleted", managedSliceDeleted.Delete(event.DeleteEvent{Object: other}), false},
		{"managed slice created", managedSliceDeleted.Create(event.CreateEvent{Object: managed}), false},
		{"managed slice updated", managedSliceDeleted.Update(event.UpdateEvent{ObjectOld: managed, ObjectNew: managed}), false},
		{"managed slice generic", managedSliceDeleted.Generic(event.GenericEvent{Object: managed}), false},
	} {
		if tc.got != tc.want {
			t.Errorf("%s: passes = %v, want %v", tc.name, tc.got, tc.want)
		}
	}
	want := []reconcile.Request{{NamespacedName: types.NamespacedName{Namespace: testNamespace, Name: testServiceName}}}
	if got := endpointSliceToRequest(t.Context(), managed); !reflect.DeepEqual(got, want) {
		t.Fatalf("slice requests = %v, want %v", got, want)
	}
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
	// A reconcile of a same-named Service elsewhere would delete that
	// namespace's slice of the same name.
	elsewhere := newRouteService(testServiceName)
	elsewhere.Namespace = "other-namespace"
	if got := r.serviceToRequest(ctx, elsewhere); len(got) != 0 {
		t.Fatalf("same-named Service in another namespace requests = %v, want none", got)
	}
}

// Only create, delete, a new UID and the start of deletion pass the Service
// watch; other updates are dropped before serviceToRequest lists Workloads.
func TestRoutedServiceChangedFilter(t *testing.T) {
	svc := newRouteService(testServiceName)
	svc.UID = "service-uid"
	svc.ResourceVersion = "1"
	update := func(mutate func(*corev1.Service)) bool {
		changed := svc.DeepCopy()
		changed.ResourceVersion = "2"
		mutate(changed)
		return routedServiceChanged.Update(event.UpdateEvent{ObjectOld: svc, ObjectNew: changed})
	}
	for _, tc := range []struct {
		name string
		got  bool
		want bool
	}{
		{"Service created", routedServiceChanged.Create(event.CreateEvent{Object: svc}), true},
		{"Service deleted", routedServiceChanged.Delete(event.DeleteEvent{Object: svc}), true},
		{"resync", routedServiceChanged.Update(event.UpdateEvent{ObjectOld: svc, ObjectNew: svc}), false},
		{"status changed", update(func(s *corev1.Service) {
			s.Status.LoadBalancer.Ingress = []corev1.LoadBalancerIngress{{IP: "192.0.2.1"}}
		}), false},
		{"labels changed", update(func(s *corev1.Service) { s.Labels = map[string]string{"app.kubernetes.io/part-of": "shop"} }), false},
		{"spec changed", update(func(s *corev1.Service) { s.Spec.Ports[0].Port = 8080 }), false},
		{"replaced (new UID)", update(func(s *corev1.Service) { s.UID = "replacement-uid" }), true},
		{"deletion started", update(func(s *corev1.Service) {
			s.DeletionTimestamp = &metav1.Time{Time: time.Now()}
		}), true},
	} {
		if tc.got != tc.want {
			t.Errorf("%s: passes = %v, want %v", tc.name, tc.got, tc.want)
		}
	}
}
