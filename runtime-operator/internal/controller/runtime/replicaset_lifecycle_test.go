package runtime

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"

	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestReplicaSetReplicaCountsAndReadiness(t *testing.T) {
	const (
		availableCase   = "all workloads available"
		scalingCase     = "scaling"
		nilReplicasCase = "nil replicas"
	)
	for _, state := range []string{nilReplicasCase, scalingCase, availableCase, "count mismatch"} {
		t.Run(state, func(t *testing.T) {
			rs := cleanupReplicaSet()
			w := cleanupWorkload(rs, "running")
			w.Status.SetConditions(condition.Available())
			var objects []client.Object
			switch state {
			case nilReplicasCase:
				rs.Spec.Replicas = nil
			case scalingCase:
				rs.Status.Conditions = nil
			case availableCase:
				objects = append(objects, w)
			}
			r := newCleanupReconciler(t, interceptor.Funcs{}, objects...)
			if state == nilReplicasCase {
				if err := r.reconcileScaleUp(t.Context(), rs); err != nil {
					t.Fatal(err)
				}
			}
			err := r.reconcileReady(t.Context(), rs)
			switch state {
			case nilReplicasCase:
				want := runtimev1alpha1.ReplicaSetStatus{}
				if err != nil || rs.Status.Replicas == nil || *rs.Status.Replicas != want {
					t.Fatalf("nil replicas = %v, replicas = %+v, want none expected or created", err, rs.Status.Replicas)
				}
			case availableCase:
				want := runtimev1alpha1.ReplicaSetStatus{Expected: 1, Current: 1, Ready: 1}
				if err != nil || rs.Status.Replicas == nil || *rs.Status.Replicas != want {
					t.Fatalf("ready = %v, replicas = %+v, want %+v", err, rs.Status.Replicas, want)
				}
			case scalingCase:
				if err == nil || !strings.Contains(err.Error(), "scaling in progress") || rs.Status.Replicas != nil {
					t.Fatalf("pending scale = %v, replicas = %+v", err, rs.Status.Replicas)
				}
			case "count mismatch":
				if err == nil || !strings.Contains(err.Error(), "expected 1 workloads, got 0") || rs.Status.CurrentReplicas != 0 {
					t.Fatalf("missing workload = %v, replicas = %+v", err, rs.Status.Replicas)
				}
			}
		})
	}
}

func TestReplicaSetScaleUpProtectsManagedAnnotations(t *testing.T) {
	const (
		forgedManagedAnnotation = "forged"
		userAnnotationValue     = "user note"
	)
	for _, deployment := range []string{"", testDeploymentName} {
		t.Run("deployment="+deployment, func(t *testing.T) {
			rs := cleanupReplicaSet()
			rs.Labels = map[string]string{workloadDeploymentNameLabel: deployment}
			rs.Spec.Template.Annotations = map[string]string{
				workloadReplicaSetGenerationAnnotation: forgedManagedAnnotation, workloadReplicaSetNameAnnotation: forgedManagedAnnotation,
				workloadDeploymentNameAnnotation: forgedManagedAnnotation, "user.example/note": userAnnotationValue,
			}
			before := rs.DeepCopy()
			r := newCleanupReconciler(t, interceptor.Funcs{})
			for range 2 {
				if err := r.reconcileScaleUp(t.Context(), rs); err != nil {
					t.Fatal(err)
				}
			}
			list := &runtimev1alpha1.WorkloadList{}
			if err := r.List(t.Context(), list); err != nil || len(list.Items) != 1 {
				t.Fatalf("workloads = %d, %v", len(list.Items), err)
			}
			want := map[string]string{
				workloadReplicaSetGenerationAnnotation: rs.Spec.Template.Hash(), workloadReplicaSetNameAnnotation: rs.Name,
				"user.example/note": userAnnotationValue,
			}
			if deployment != "" {
				want[workloadDeploymentNameAnnotation] = deployment
			}
			if !reflect.DeepEqual(list.Items[0].Annotations, want) || !reflect.DeepEqual(rs, before) {
				t.Fatalf("annotations = %v, want %v; template changed = %v", list.Items[0].Annotations, want, !reflect.DeepEqual(rs, before))
			}
		})
	}
}

func TestReplicaSetScaleReadAndCreateFailuresRetry(t *testing.T) {
	const readyListCase = "ready list"
	for _, operation := range []string{testListOperation, testCreateOperation, readyListCase} {
		t.Run(operation, func(t *testing.T) {
			rs := cleanupReplicaSet()
			cause := errors.New("API unavailable")
			fail := true
			r := newCleanupReconciler(t, interceptor.Funcs{
				List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
					if fail && operation != testCreateOperation {
						return cause
					}
					return c.List(ctx, list, opts...)
				},
				Create: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.CreateOption) error {
					if fail && operation == testCreateOperation {
						return cause
					}
					return c.Create(ctx, obj, opts...)
				},
			})
			reconcile := r.reconcileScaleUp
			if operation == readyListCase {
				reconcile = r.reconcileReady
			}
			if err := reconcile(t.Context(), rs); err == nil || !strings.Contains(err.Error(), cause.Error()) {
				t.Fatalf("reconcile = %v, want API error", err)
			}
			if operation == readyListCase && rs.Status.Replicas != nil {
				t.Fatalf("failed ready read reported replicas: %+v", rs.Status.Replicas)
			}
			fail = false
			for range 2 {
				if err := r.reconcileScaleUp(t.Context(), rs); err != nil {
					t.Fatal(err)
				}
			}
			list := &runtimev1alpha1.WorkloadList{}
			if err := r.List(t.Context(), list); err != nil || len(list.Items) != 1 {
				t.Fatalf("retry workloads = %d, %v", len(list.Items), err)
			}
			err := r.reconcileReady(t.Context(), rs)
			want := runtimev1alpha1.ReplicaSetStatus{Expected: 1, Current: 1, Unavailable: 1}
			if err == nil || !strings.Contains(err.Error(), "1 workloads are not available") ||
				rs.Status.Replicas == nil || *rs.Status.Replicas != want {
				t.Fatalf("ready after retry = %v, replicas = %+v, want %+v", err, rs.Status.Replicas, want)
			}
		})
	}
}
