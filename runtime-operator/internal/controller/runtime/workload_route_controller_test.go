package runtime

import (
	"context"
	"net"
	"testing"

	corev1 "k8s.io/api/core/v1"
	discoveryv1 "k8s.io/api/discovery/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	// testNamespace is shared with workload_deployment_controller_test.go.
	testServiceName = "wl-route-test"
	testHostID      = "host-abc"
	testPodIP       = "10.244.0.42"
)

func newRouteTestScheme(t *testing.T) *runtime.Scheme {
	t.Helper()
	s := runtime.NewScheme()
	if err := corev1.AddToScheme(s); err != nil {
		t.Fatalf("add corev1: %v", err)
	}
	if err := discoveryv1.AddToScheme(s); err != nil {
		t.Fatalf("add discoveryv1: %v", err)
	}
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	return s
}

// newRouteReconciler wires the same field indexes that SetupWithManager
// would register in production, so r.List(...client.MatchingFields{...})
// resolves under the fake client.
func newRouteReconciler(t *testing.T, objs ...client.Object) *WorkloadRouteReconciler {
	t.Helper()
	s := newRouteTestScheme(t)
	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.Workload{}, workloadKubernetesServiceIndex,
			func(obj client.Object) []string {
				w, ok := obj.(*runtimev1alpha1.Workload)
				if !ok || w.Spec.Kubernetes == nil || w.Spec.Kubernetes.Service == nil {
					return nil
				}
				if w.Spec.Kubernetes.Service.Name == "" {
					return nil
				}
				return []string{w.Spec.Kubernetes.Service.Name}
			}).
		WithIndex(&runtimev1alpha1.Host{}, hostIDIndex,
			func(obj client.Object) []string {
				h, ok := obj.(*runtimev1alpha1.Host)
				if !ok || h.HostID == "" {
					return nil
				}
				return []string{h.HostID}
			}).
		Build()
	return &WorkloadRouteReconciler{
		Client:            c,
		Scheme:            s,
		OperatorNamespace: testNamespace,
	}
}

func newRouteService(name string) *corev1.Service {
	return &corev1.Service{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: name},
		Spec:       corev1.ServiceSpec{Ports: []corev1.ServicePort{{Port: 80}}},
	}
}

func newAvailableWorkload(name, svcName, hostID string) *runtimev1alpha1.Workload {
	w := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: name},
		Spec: runtimev1alpha1.WorkloadSpec{
			Kubernetes: &runtimev1alpha1.KubernetesSpec{
				Service: &runtimev1alpha1.KubernetesServiceRef{Name: svcName},
			},
		},
		Status: runtimev1alpha1.WorkloadStatus{
			HostID: hostID,
		},
	}
	w.Status.SetConditions(condition.Available())
	return w
}

func newHost(name, id, hostname string, port uint32) *runtimev1alpha1.Host {
	return &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: name},
		HostID:     id,
		Hostname:   hostname,
		HTTPPort:   port,
	}
}

// newHostPod constructs a Pod that simulates the wasmCloud host container.
// The Pod carries HostPodLabel, exposes podIP via Status.PodIP, and pins
// `Spec.Hostname` to the same value the wasmCloud host process reports in
// its heartbeat's req.Hostname (= host.Hostname).
func newHostPod(podName, specHostname, podIP string) *corev1.Pod {
	return &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testNamespace,
			Name:      podName,
			Labels:    map[string]string{HostPodLabel: "default"},
		},
		Spec:   corev1.PodSpec{Hostname: specHostname},
		Status: corev1.PodStatus{PodIP: podIP},
	}
}

func runReconcile(t *testing.T, r *WorkloadRouteReconciler, svcName string) {
	t.Helper()
	_, err := r.Reconcile(context.Background(), ctrl.Request{
		NamespacedName: types.NamespacedName{Namespace: testNamespace, Name: svcName},
	})
	if err != nil {
		t.Fatalf("reconcile: %v", err)
	}
}

func getSlice(t *testing.T, r *WorkloadRouteReconciler, svcName string) *discoveryv1.EndpointSlice {
	t.Helper()
	slice := &discoveryv1.EndpointSlice{}
	err := r.Get(context.Background(), types.NamespacedName{
		Namespace: testNamespace,
		Name:      endpointSliceName(svcName),
	}, slice)
	if err != nil {
		if apierrors.IsNotFound(err) {
			return nil
		}
		t.Fatalf("get slice: %v", err)
	}
	return slice
}

// TestWorkloadRouteReconcile_NonIPHostname_ResolvesViaPodIP is the regression
// pin for the bug captured in this repo's
// docs/initiatives/2026-05-09-wasmcloud-workload-route-pod-ip-bug.md
// and reverified at runtime-operator v2.1.0.
//
// Setup: a Host whose `Hostname` is a non-IP string (e.g. the OS
// hostname `wasmcloud-host` that this repo's host-hostname-patch
// pins, or the default pod-name shape `hostgroup-default-…` that
// kasm/kind setups end up with). A backing Pod carrying
// `HostPodLabel` exposes the real pod IP via `Status.PodIP`.
//
// Expected: the EndpointSlice that the reconciler creates carries
// the Pod's IP — the only value that `AddressType: IPv4` slices
// route via kube-proxy. With the historical code path
// (`podIP := host.Hostname`) this test fails because the slice
// ends up carrying the unparseable string `wasmcloud-host`,
// which the real Kubernetes API rejects with the
// `Invalid value: "wasmcloud-host": must be a valid IPv4 address`
// error users see in production.
func TestWorkloadRouteReconcile_NonIPHostname_ResolvesViaPodIP(t *testing.T) {
	const osHostname = "wasmcloud-host"
	svc := newRouteService(testServiceName)
	wl := newAvailableWorkload("wl-1", testServiceName, testHostID)
	host := newHost("hostgroup-default-abc", testHostID, osHostname, 9191)
	pod := newHostPod("hostgroup-default-abcxyz", osHostname, testPodIP)

	r := newRouteReconciler(t, svc, wl, host, pod)
	runReconcile(t, r, testServiceName)

	slice := getSlice(t, r, testServiceName)
	if slice == nil {
		t.Fatalf("EndpointSlice was not created for service %q", testServiceName)
		return
	}
	if got, want := slice.AddressType, discoveryv1.AddressTypeIPv4; got != want {
		t.Fatalf("AddressType = %q, want %q", got, want)
	}
	if len(slice.Endpoints) != 1 || len(slice.Endpoints[0].Addresses) != 1 {
		t.Fatalf("expected exactly one endpoint with one address; got %#v", slice.Endpoints)
	}
	addr := slice.Endpoints[0].Addresses[0]
	if net.ParseIP(addr) == nil {
		t.Fatalf("EndpointSlice address %q is not a valid IP; an IPv4-typed slice with a non-IP address is what the upstream K8s API rejects with `must be a valid IPv4 address`", addr)
	}
	if addr != testPodIP {
		t.Fatalf("EndpointSlice address = %q, want %q (the Pod's Status.PodIP)", addr, testPodIP)
	}
}

// TestWorkloadRouteReconcile_IPHostname_BackwardCompat covers the
// upstream-documented contract where Host.Hostname already
// contains the pod IP (e.g. a host process correctly wired via
// Downward API). Pre-fix code uses it directly. The fix must
// continue to use it directly when it IS an IP — no spurious
// Pod lookup, no behaviour change.
func TestWorkloadRouteReconcile_IPHostname_BackwardCompat(t *testing.T) {
	const podIP = "10.244.0.99"
	svc := newRouteService(testServiceName)
	wl := newAvailableWorkload("wl-1", testServiceName, testHostID)
	host := newHost("hostgroup-default-abc", testHostID, podIP, 9191)
	// Note: no Pod object seeded. The IP path must work without one.

	r := newRouteReconciler(t, svc, wl, host)
	runReconcile(t, r, testServiceName)

	slice := getSlice(t, r, testServiceName)
	if slice == nil {
		t.Fatalf("EndpointSlice was not created")
		return
	}
	if len(slice.Endpoints) != 1 || len(slice.Endpoints[0].Addresses) != 1 {
		t.Fatalf("expected exactly one endpoint with one address; got %#v", slice.Endpoints)
	}
	if got := slice.Endpoints[0].Addresses[0]; got != podIP {
		t.Fatalf("EndpointSlice address = %q, want %q", got, podIP)
	}
}

// TestWorkloadRouteReconcile_NonIPHostname_NoMatchingPod confirms the
// "skip-not-fail" behaviour when Hostname isn't an IP and no Pod
// matches. The reconciler should skip the workload (log + continue)
// rather than emit a broken slice. With no endpoints left, the
// reconciler deletes any pre-existing slice — that's the existing
// empty-set branch and must keep working.
func TestWorkloadRouteReconcile_NonIPHostname_NoMatchingPod(t *testing.T) {
	svc := newRouteService(testServiceName)
	wl := newAvailableWorkload("wl-1", testServiceName, testHostID)
	host := newHost("hostgroup-default-abc", testHostID, "wasmcloud-host", 9191)
	// No Pod seeded.

	r := newRouteReconciler(t, svc, wl, host)
	runReconcile(t, r, testServiceName)

	if slice := getSlice(t, r, testServiceName); slice != nil {
		t.Fatalf("expected no EndpointSlice (workload skipped); got %d endpoints", len(slice.Endpoints))
	}
}
