package runtime

import (
	"reflect"
	"testing"

	corev1 "k8s.io/api/core/v1"
	discoveryv1 "k8s.io/api/discovery/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestWorkloadRouteIsolatesSameNamedServicesByNamespace(t *testing.T) {
	namespaces := []string{"route-tenant-a", "route-tenant-b"}
	ips := []string{testPodIP, routeSecondIP}
	services := make([]*corev1.Service, len(namespaces))
	workloads := make([]*runtimev1alpha1.Workload, len(namespaces))
	hosts := make([]*runtimev1alpha1.Host, len(namespaces))
	objects := make([]client.Object, 0, 3*len(namespaces))
	for i, namespace := range namespaces {
		services[i] = newRouteService(testServiceName)
		services[i].Namespace = namespace
		services[i].UID = types.UID(namespace + "-service")
		workloads[i] = newAvailableWorkload("ready", testServiceName, namespace+"-host-id")
		workloads[i].Namespace = namespace
		hosts[i] = newHost(namespace+"-host", workloads[i].Status.HostID, ips[i], routeTestPort)
		objects = append(objects, services[i], workloads[i], hosts[i])
	}
	r := newRouteReconciler(t, objects...)
	for _, svc := range services {
		if err := r.Get(t.Context(), client.ObjectKeyFromObject(svc), svc); err != nil {
			t.Fatal(err)
		}
	}
	get := func(namespace string) *discoveryv1.EndpointSlice {
		t.Helper()
		slice := &discoveryv1.EndpointSlice{}
		err := r.Get(t.Context(), client.ObjectKey{Namespace: namespace, Name: endpointSliceName(testServiceName)}, slice)
		if apierrors.IsNotFound(err) {
			return nil
		}
		if err != nil {
			t.Fatal(err)
		}
		return slice
	}
	reconcile := func(i int) {
		t.Helper()
		peerNamespace := namespaces[1-i]
		peerBefore := get(peerNamespace)
		result, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: client.ObjectKeyFromObject(services[i])})
		if err != nil || result != (ctrl.Result{}) {
			t.Fatalf("reconcile %s = %+v, %v", namespaces[i], result, err)
		}
		if peerAfter := get(peerNamespace); !reflect.DeepEqual(peerAfter, peerBefore) {
			t.Fatalf("reconciling %s changed the slice in %s: before=%+v after=%+v", namespaces[i], peerNamespace, peerBefore, peerAfter)
		}
		for _, before := range services {
			after := &corev1.Service{}
			if err := r.Get(t.Context(), client.ObjectKeyFromObject(before), after); err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(after, before) {
				t.Fatalf("reconciling %s changed Service %s/%s", namespaces[i], before.Namespace, before.Name)
			}
		}
	}

	for i, namespace := range namespaces {
		reconcile(i)
		assertRouteSlice(t, get(namespace), services[i], ips[i])
	}
	for i, namespace := range namespaces {
		host := hosts[i]
		if err := r.Get(t.Context(), client.ObjectKeyFromObject(host), host); err != nil {
			t.Fatal(err)
		}
		host.Hostname = []string{"10.244.0.77", "10.244.0.88"}[i]
		if err := r.Update(t.Context(), host); err != nil {
			t.Fatal(err)
		}
		reconcile(i)
		assertRouteSlice(t, get(namespace), services[i], host.Hostname)
	}
	for i, namespace := range namespaces {
		workload := workloads[i]
		if err := r.Get(t.Context(), client.ObjectKeyFromObject(workload), workload); err != nil {
			t.Fatal(err)
		}
		workload.Status.SetConditions(condition.Unavailable())
		if err := r.Update(t.Context(), workload); err != nil {
			t.Fatal(err)
		}
		reconcile(i)
		if slice := get(namespace); slice != nil {
			t.Fatalf("unready workload retained slice in %s: %+v", namespace, slice)
		}
	}
}
