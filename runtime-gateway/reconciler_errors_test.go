package main

import (
	"context"
	"errors"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

type failingRegistry struct{ err error }

func (r failingRegistry) RegisterHost(context.Context, types.NamespacedName, string, string, int) error {
	return r.err
}
func (r failingRegistry) DeregisterHost(context.Context, types.NamespacedName) error {
	return r.err
}
func (r failingRegistry) RegisterWorkload(context.Context, types.NamespacedName, string, string, string) error {
	return r.err
}
func (r failingRegistry) DeregisterWorkload(context.Context, types.NamespacedName) error {
	return r.err
}

func TestReconcilersPropagateErrors(t *testing.T) {
	want := errors.New("retry this operation")
	for _, kind := range []string{"host", "workload"} {
		for _, failure := range []string{"get", "patch", "register", "deregister", "missing"} {
			t.Run(kind+"/"+failure, func(t *testing.T) {
				var obj client.Object
				if kind == "workload" {
					w := availableWorkload("route")
					if failure == "deregister" {
						w.Status.SetConditions(condition.Unavailable())
					}
					if failure == "patch" {
						w.Finalizers = []string{gatewayWorkloadFinalizerName}
					}
					obj = w
				} else {
					h := &runtimev1alpha1.Host{ObjectMeta: metav1.ObjectMeta{Namespace: "ns", Name: "host"}, HostID: "id", Hostname: "backend", HTTPPort: 8080}
					if failure != "deregister" {
						h.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
					}
					if failure == "patch" {
						h.Finalizers = []string{gatewayHostFinalizerName}
					}
					obj = h
				}
				funcs := interceptor.Funcs{}
				if failure == "get" {
					funcs.Get = func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
						return want
					}
				}
				if failure == "patch" {
					funcs.Patch = func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
						return want
					}
				}
				builder := fake.NewClientBuilder().WithScheme(testScheme(t)).WithInterceptorFuncs(funcs)
				if failure != "missing" {
					builder.WithObjects(obj)
				}
				c := builder.Build()
				request := ctrl.Request{NamespacedName: client.ObjectKeyFromObject(obj)}
				registryError := want
				if failure == "get" || failure == "patch" {
					registryError = errors.New("registry called after an API failure")
				}
				var err error
				if kind == "workload" {
					r := &WorkloadReconciler{Client: c, Registry: failingRegistry{registryError}}
					_, err = r.Reconcile(t.Context(), request)
				} else {
					r := &HostReconciler{Client: c, Registry: failingRegistry{registryError}}
					_, err = r.Reconcile(t.Context(), request)
				}
				if !errors.Is(err, want) {
					t.Fatalf("Reconcile() = %v, want %v", err, want)
				}
			})
		}
	}
}

func TestWorkloadReconcilerReadinessTransitions(t *testing.T) {
	for _, state := range []string{"unplaced", "no hostname", "unavailable"} {
		t.Run(state, func(t *testing.T) {
			workload := availableWorkload("route")
			switch state {
			case "unplaced":
				workload.Status.SetConditions(condition.UnknownCondition(runtimev1alpha1.WorkloadConditionPlacement, "Pending", "pending"))
			case "no hostname":
				workload.Spec.HostInterfaces = nil
			case "unavailable":
				workload.Status.SetConditions(condition.Unavailable())
			}
			key := client.ObjectKeyFromObject(workload)
			registry := newRecordingRegistry()
			if state != "unplaced" {
				registry.workloads[key] = testRoute()
			}
			c := fake.NewClientBuilder().WithScheme(testScheme(t)).WithObjects(workload).Build()
			r := &WorkloadReconciler{Client: c, Registry: registry}
			if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
				t.Fatal(err)
			}
			if len(registry.workloads) != 0 {
				t.Fatalf("unroutable workload remains registered: %v", registry.workloads)
			}
		})
	}
}

func TestHostReconcilerReadinessTransitions(t *testing.T) {
	for _, state := range []string{"available", "unavailable", "deleting"} {
		t.Run(state, func(t *testing.T) {
			host := &runtimev1alpha1.Host{ObjectMeta: metav1.ObjectMeta{Namespace: "ns", Name: "host"}, HostID: "id", Hostname: "backend", HTTPPort: 8080}
			host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
			if state == "unavailable" {
				host.Status.SetConditions(condition.Unavailable())
			}
			if state == "deleting" {
				now := metav1.Now()
				host.DeletionTimestamp = &now
				host.Finalizers = []string{"other-controller"}
			}
			key := client.ObjectKeyFromObject(host)
			registry := newRecordingRegistry()
			registry.hosts[key] = "stale-id"
			c := fake.NewClientBuilder().WithScheme(testScheme(t)).WithObjects(host).Build()
			r := &HostReconciler{Client: c, Registry: registry}
			if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
				t.Fatal(err)
			}
			if _, found := registry.hosts[key]; found != (state == "available") {
				t.Fatalf("host registration present=%v for %s host", found, state)
			}
			if state == "available" && registry.hosts[key] != host.HostID {
				t.Fatalf("host registration = %q, want %q", registry.hosts[key], host.HostID)
			}
		})
	}
}
