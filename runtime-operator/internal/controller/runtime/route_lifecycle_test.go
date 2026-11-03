package runtime

import (
	"context"
	"reflect"
	"slices"
	"sort"
	"strings"
	"testing"

	corev1 "k8s.io/api/core/v1"
	discoveryv1 "k8s.io/api/discovery/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/util/validation"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
)

const (
	routeTestPort    = 9191
	routeSecondIP    = "10.244.0.99"
	routePodHostname = "route-host-pod"
	routeServiceUID  = "route-service-uid"
)

func assertRouteSlice(t *testing.T, slice *discoveryv1.EndpointSlice, svc *corev1.Service, ips ...string) {
	t.Helper()
	if slice == nil {
		t.Fatal("expected an EndpointSlice")
	}
	if slice.Namespace != svc.Namespace || slice.Name != endpointSliceName(svc.Name) {
		t.Fatalf("slice key = %s/%s, want %s/%s", slice.Namespace, slice.Name, svc.Namespace, endpointSliceName(svc.Name))
	}
	if slice.AddressType != discoveryv1.AddressTypeIPv4 || !metav1.IsControlledBy(slice, svc) {
		t.Fatalf("slice address type or owner = %s, %v", slice.AddressType, slice.OwnerReferences)
	}
	wantLabels := map[string]string{discoveryv1.LabelServiceName: svc.Name, routeManagerLabel: "true"}
	if !reflect.DeepEqual(slice.Labels, wantLabels) {
		t.Fatalf("slice labels = %v, want %v", slice.Labels, wantLabels)
	}
	if len(slice.Ports) != 1 {
		t.Fatalf("slice ports = %v, want one HTTP port", slice.Ports)
	}
	port := slice.Ports[0]
	if port.Name == nil || *port.Name != "http" || port.Port == nil || *port.Port != routeTestPort || port.Protocol == nil || *port.Protocol != corev1.ProtocolTCP {
		t.Fatalf("slice port = %+v", port)
	}
	got := make([]string, 0, len(slice.Endpoints))
	for _, endpoint := range slice.Endpoints {
		if len(endpoint.Addresses) != 1 || endpoint.Conditions.Ready == nil || !*endpoint.Conditions.Ready || endpoint.Conditions.Serving == nil || !*endpoint.Conditions.Serving {
			t.Fatalf("invalid serving endpoint = %+v", endpoint)
		}
		got = append(got, endpoint.Addresses[0])
	}
	sort.Strings(got)
	sort.Strings(ips)
	if !reflect.DeepEqual(got, ips) {
		t.Fatalf("endpoint addresses = %v, want %v", got, ips)
	}
}

func TestWorkloadRouteEndpointLifecycle(t *testing.T) {
	svc := newRouteService(testServiceName)
	svc.UID = routeServiceUID
	unrelatedService := newRouteService("route-unrelated")
	first := newAvailableWorkload("first", svc.Name, testHostID)
	duplicate := newAvailableWorkload("duplicate", svc.Name, testHostID)
	second := newAvailableWorkload("second", svc.Name, "second-host-id")
	otherService := newAvailableWorkload("other-service", unrelatedService.Name, "unrelated-host")
	otherNamespace := newAvailableWorkload("other-namespace", svc.Name, "unrelated-host")
	otherNamespace.Namespace = "route-tenant"
	unlabelledPod := newHostPod("unlabelled", routePodHostname, "10.244.0.1")
	unlabelledPod.Labels = nil
	r := newRouteReconciler(t, svc, unrelatedService, first, duplicate, second, otherService, otherNamespace,
		newHost("first-host", testHostID, testPodIP, routeTestPort),
		newHost("second-host", second.Status.HostID, routePodHostname, routeTestPort),
		newHost("unrelated-host", "unrelated-host", "10.244.0.2", 8080),
		unlabelledPod, newHostPod(routePodHostname, "", routeSecondIP),
	)
	runReconcile(t, r, svc.Name)
	initial := getSlice(t, r, svc.Name)
	assertRouteSlice(t, initial, svc, testPodIP, routeSecondIP)
	if unrelated := getSlice(t, r, unrelatedService.Name); unrelated != nil {
		t.Fatal("reconciliation created a slice for another Service")
	}

	slices.Reverse(initial.Endpoints)
	if err := r.Update(t.Context(), initial); err != nil {
		t.Fatal(err)
	}
	base := r.Client.(client.WithWatch)
	updates := 0
	r.Client = interceptor.NewClient(base, interceptor.Funcs{
		Update: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.UpdateOption) error {
			updates++
			return c.Update(ctx, obj, opts...)
		},
	})
	for range 2 {
		runReconcile(t, r, svc.Name)
	}
	if updates != 0 {
		t.Fatalf("equivalent endpoints caused %d writes", updates)
	}
	for i, workload := range []client.Object{duplicate, first} {
		current := first.DeepCopy()
		if err := base.Get(t.Context(), client.ObjectKeyFromObject(workload), current); err != nil {
			t.Fatal(err)
		}
		current.Status.SetConditions(condition.Unavailable())
		if err := base.Update(t.Context(), current); err != nil {
			t.Fatal(err)
		}
		if i == 0 {
			runReconcile(t, r, svc.Name)
			assertRouteSlice(t, getSlice(t, r, svc.Name), svc, testPodIP, routeSecondIP)
			if updates != 0 {
				t.Fatalf("one unready workload on a shared host caused %d writes", updates)
			}
		}
	}
	runReconcile(t, r, svc.Name)
	assertRouteSlice(t, getSlice(t, r, svc.Name), svc, routeSecondIP)
	if updates != 1 {
		t.Fatalf("removing a host endpoint caused %d writes, want one", updates)
	}
	if err := base.Get(t.Context(), client.ObjectKeyFromObject(second), second); err != nil {
		t.Fatal(err)
	}
	second.Status.SetConditions(condition.Unavailable())
	if err := base.Update(t.Context(), second); err != nil {
		t.Fatal(err)
	}
	for range 2 {
		runReconcile(t, r, svc.Name)
		if got := getSlice(t, r, svc.Name); got != nil {
			t.Fatalf("empty service retained endpoints: %v", got.Endpoints)
		}
	}
}

func TestWorkloadRouteRepairsEndpointSliceDrift(t *testing.T) {
	for _, field := range []string{"labels", "address type", "port", "endpoints"} {
		t.Run(field, func(t *testing.T) {
			svc := newRouteService(testServiceName)
			svc.UID = routeServiceUID
			r := newRouteReconciler(t, svc,
				newAvailableWorkload("ready", svc.Name, testHostID),
				newHost("ready-host", testHostID, testPodIP, routeTestPort),
			)
			runReconcile(t, r, svc.Name)
			stale := getSlice(t, r, svc.Name)
			switch field {
			case "labels":
				stale.Labels = map[string]string{routeManagerLabel: "false"}
			case "address type":
				stale.AddressType = discoveryv1.AddressTypeFQDN
			case "port":
				stale.Ports[0].Port = new(int32(8080))
			case "endpoints":
				stale.Endpoints = []discoveryv1.Endpoint{{}, {Addresses: []string{"10.244.0.1"}}}
			}
			if err := r.Update(t.Context(), stale); err != nil {
				t.Fatal(err)
			}
			runReconcile(t, r, svc.Name)
			assertRouteSlice(t, getSlice(t, r, svc.Name), svc, testPodIP)
		})
	}
}

func TestWorkloadRouteEndpointSliceNames(t *testing.T) {
	names := []string{testServiceName, strings.Repeat("s", 54), strings.Repeat("s", 63), strings.Repeat("s", 62) + "x"}
	seen := make(map[string]bool)
	for _, service := range names {
		name := endpointSliceName(service)
		if problems := validation.IsDNS1123Label(name); len(problems) != 0 {
			t.Fatalf("invalid EndpointSlice name %q: %v", name, problems)
		}
		if seen[name] || endpointSliceName(service) != name {
			t.Fatalf("EndpointSlice name %q is not stable and distinct", name)
		}
		seen[name] = true
	}
}
