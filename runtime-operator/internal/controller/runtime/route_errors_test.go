package runtime

import (
	"context"
	"errors"
	"reflect"
	"testing"

	corev1 "k8s.io/api/core/v1"
	discoveryv1 "k8s.io/api/discovery/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func staleRouteSlice() *discoveryv1.EndpointSlice {
	return &discoveryv1.EndpointSlice{
		ObjectMeta:  metav1.ObjectMeta{Namespace: testNamespace, Name: endpointSliceName(testServiceName)},
		AddressType: discoveryv1.AddressTypeIPv4,
		Endpoints:   []discoveryv1.Endpoint{{Addresses: []string{"10.244.0.1"}}},
	}
}

func TestWorkloadRouteAPIFailuresPreserveExistingSlice(t *testing.T) {
	const (
		serviceRead   = "get service"
		workloadsRead = "list workloads"
		sliceRead     = "get slice"
		sliceCreate   = "create slice"
		sliceUpdate   = "update slice"
		sliceDelete   = "delete slice"
	)
	for _, stage := range []string{serviceRead, workloadsRead, sliceRead, sliceCreate, sliceUpdate, sliceDelete} {
		t.Run(stage, func(t *testing.T) {
			objects := []client.Object{newRouteService(testServiceName), newHost("ready-host", testHostID, testPodIP, routeTestPort)}
			if stage != sliceDelete {
				objects = append(objects, newAvailableWorkload("ready", testServiceName, testHostID))
			}
			if stage != sliceCreate {
				objects = append(objects, staleRouteSlice())
			}
			r := newRouteReconciler(t, objects...)
			before := getSlice(t, r, testServiceName)
			base := r.Client.(client.WithWatch)
			want := errors.New("API request refused")
			r.Client = interceptor.NewClient(base, interceptor.Funcs{
				Get: func(ctx context.Context, c client.WithWatch, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
					switch obj.(type) {
					case *corev1.Service:
						if stage == serviceRead {
							return want
						}
					case *discoveryv1.EndpointSlice:
						if stage == sliceRead {
							return want
						}
					}
					return c.Get(ctx, key, obj, opts...)
				},
				List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
					if _, ok := list.(*runtimev1alpha1.WorkloadList); ok && stage == workloadsRead {
						return want
					}
					return c.List(ctx, list, opts...)
				},
				Create: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.CreateOption) error {
					if stage == sliceCreate {
						return want
					}
					return c.Create(ctx, obj, opts...)
				},
				Update: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.UpdateOption) error {
					if stage == sliceUpdate {
						return want
					}
					return c.Update(ctx, obj, opts...)
				},
				Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
					if stage == sliceDelete {
						return want
					}
					return c.Delete(ctx, obj, opts...)
				},
			})
			result, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: client.ObjectKey{Namespace: testNamespace, Name: testServiceName}})
			if !errors.Is(err, want) || result != (ctrl.Result{}) {
				t.Fatalf("Reconcile = %+v, %v; want the API error", result, err)
			}
			r.Client = base
			if after := getSlice(t, r, testServiceName); !reflect.DeepEqual(after, before) {
				t.Fatalf("failed reconciliation changed the stored slice: before=%+v after=%+v", before, after)
			}
		})
	}
}

func TestWorkloadRouteRemovesUnroutableEndpoints(t *testing.T) {
	for _, state := range []string{"unavailable", "no host ID", "absent host", "empty hostname", "zero port", "pending pod", "host outside operator namespace"} {
		t.Run(state, func(t *testing.T) {
			workload := newAvailableWorkload("ready", testServiceName, testHostID)
			host := newHost("ready-host", testHostID, testPodIP, routeTestPort)
			objects := []client.Object{newRouteService(testServiceName), workload, staleRouteSlice()}
			switch state {
			case "unavailable":
				workload.Status.SetConditions(condition.Unavailable())
			case "no host ID":
				workload.Status.HostID = ""
			case "empty hostname":
				host.Hostname = ""
			case "zero port":
				host.HTTPPort = 0
			case "pending pod":
				host.Hostname = routePodHostname
				objects = append(objects, newHostPod(routePodHostname, "", ""))
			case "host outside operator namespace":
				host.Namespace = "route-hosts-elsewhere"
			}
			if state != "absent host" {
				objects = append(objects, host)
			}
			r := newRouteReconciler(t, objects...)
			for range 2 {
				runReconcile(t, r, testServiceName)
				if got := getSlice(t, r, testServiceName); got != nil {
					t.Fatalf("unroutable service retained stale endpoints: %v", got.Endpoints)
				}
			}
		})
	}
}

func TestWorkloadRouteIgnoresEmptyAndAbsentServices(t *testing.T) {
	for _, name := range []string{"", testServiceName} {
		t.Run(name, func(t *testing.T) {
			r := newRouteReconciler(t, staleRouteSlice())
			base := r.Client.(client.WithWatch)
			r.Client = interceptor.NewClient(base, interceptor.Funcs{
				List: func(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error {
					t.Fatal("queried workloads without a Service")
					return nil
				},
				Delete: func(context.Context, client.WithWatch, client.Object, ...client.DeleteOption) error {
					t.Fatal("deleted a slice without a Service")
					return nil
				},
			})
			runReconcile(t, r, name)
		})
	}
}

func TestWorkloadRouteLookupFailures(t *testing.T) {
	for _, resource := range []string{"hosts", "pods"} {
		t.Run(resource, func(t *testing.T) {
			r := newRouteReconciler(t)
			want := errors.New("lookup failed")
			r.Client = interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{
				List: func(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error { return want },
			})
			var err error
			if resource == "hosts" {
				_, err = r.findHostByID(t.Context(), testHostID)
			} else {
				_, err = r.findPodByHostname(t.Context(), routePodHostname)
			}
			if !errors.Is(err, want) {
				t.Fatalf("lookup = %v, want %v", err, want)
			}
		})
	}
}

type routeIndexManager struct {
	ctrl.Manager
	indexer client.FieldIndexer
}

func (m routeIndexManager) GetFieldIndexer() client.FieldIndexer { return m.indexer }

type routeTestIndexer struct {
	register func(context.Context, client.Object, string, client.IndexerFunc) error
}

func (i routeTestIndexer) IndexField(ctx context.Context, obj client.Object, field string, fn client.IndexerFunc) error {
	return i.register(ctx, obj, field, fn)
}

func TestWorkloadRouteRegistersServiceIndex(t *testing.T) {
	want := errors.New("index registration failed")
	var index client.IndexerFunc
	mgr := routeIndexManager{indexer: routeTestIndexer{register: func(_ context.Context, obj client.Object, field string, fn client.IndexerFunc) error {
		if _, ok := obj.(*runtimev1alpha1.Workload); !ok || field != workloadKubernetesServiceIndex {
			t.Fatalf("registered index %q on %T", field, obj)
		}
		index = fn
		return want
	}}}
	r := &WorkloadRouteReconciler{}
	if err := r.SetupWithManager(mgr); !errors.Is(err, want) {
		t.Fatalf("SetupWithManager = %v, want the index error", err)
	}
	if index == nil {
		t.Fatal("service index was not registered")
	}
	for _, tc := range []struct {
		name string
		obj  client.Object
		want []string
	}{
		{"wrong kind", &corev1.Pod{}, nil},
		{"no Kubernetes config", &runtimev1alpha1.Workload{}, nil},
		{"no Service", &runtimev1alpha1.Workload{Spec: runtimev1alpha1.WorkloadSpec{Kubernetes: &runtimev1alpha1.KubernetesSpec{}}}, nil},
		{"empty Service", newAvailableWorkload("empty-service", "", testHostID), nil},
		{"configured Service", newAvailableWorkload("routed", testServiceName, testHostID), []string{testServiceName}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := index(tc.obj); !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("service index = %v, want %v", got, tc.want)
			}
		})
	}
}
