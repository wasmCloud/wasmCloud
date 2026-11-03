package runtime

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const cleanupPreviousHash = "previous-hash"

func cleanupReplicaSet() *runtimev1alpha1.WorkloadReplicaSet {
	rs := baseReplicaSet(testReplicaSetName)
	rs.UID = testReplicaSetUID
	rs.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp),
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleDown),
	)
	return rs
}

func cleanupWorkload(rs *runtimev1alpha1.WorkloadReplicaSet, name string) *runtimev1alpha1.Workload {
	return &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{
		Namespace: rs.Namespace, Name: name,
		Annotations: map[string]string{workloadReplicaSetGenerationAnnotation: rs.Spec.Template.Hash()},
		OwnerReferences: []metav1.OwnerReference{{
			APIVersion: runtimev1alpha1.GroupVersion.String(), Kind: rs.Kind,
			Name: rs.Name, UID: rs.UID, Controller: ptrTrue(),
		}},
	}}
}

func failedCleanupWorkload(rs *runtimev1alpha1.WorkloadReplicaSet, name string, failedAt time.Time) *runtimev1alpha1.Workload {
	w := cleanupWorkload(rs, name)
	w.Status.Conditions = []condition.Condition{{
		Type: runtimev1alpha1.WorkloadConditionSync, Status: condition.ConditionFalse,
		LastTransitionTime: metav1.NewTime(failedAt),
	}}
	return w
}

func newCleanupReconciler(t *testing.T, funcs interceptor.Funcs, objects ...client.Object) *WorkloadReplicaSetReconciler {
	t.Helper()
	s := newScaleTestScheme(t)
	gvk := runtimev1alpha1.GroupVersion.WithKind("WorkloadReplicaSet")
	c := fake.NewClientBuilder().WithScheme(s).WithObjects(objects...).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, replicaSetOwnerIndexValue(gvk)).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetHashIndex, replicaSetHashIndexValue(gvk)).
		WithInterceptorFuncs(funcs).Build()
	return &WorkloadReplicaSetReconciler{Client: c, Scheme: s}
}

func TestReplicaSetCleanupPreservesRecoverableAndUnrelatedWorkloads(t *testing.T) {
	rs := cleanupReplicaSet()
	now := time.Now()
	healthy := cleanupWorkload(rs, "healthy")
	healthy.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	expired := failedCleanupWorkload(rs, "expired", now.Add(-time.Hour))
	recent := failedCleanupWorkload(rs, "recent", now)
	pending := cleanupWorkload(rs, "pending")
	synced := cleanupWorkload(rs, "synced")
	synced.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionSync))
	previous := cleanupWorkload(rs, "previous")
	previous.Annotations[workloadReplicaSetGenerationAnnotation] = cleanupPreviousHash
	unversioned := cleanupWorkload(rs, "unversioned")
	unversioned.Annotations = nil
	deleting := previous.DeepCopy()
	deleting.Name = "deletion-started"
	deletedAt := metav1.NewTime(now.Add(-time.Minute))
	deleting.DeletionTimestamp = &deletedAt
	deleting.Finalizers = []string{"test.example/hold"}
	predecessor := expired.DeepCopy()
	predecessor.Name = "predecessor"
	predecessor.OwnerReferences[0].UID = "previous-owner"
	otherNamespace := expired.DeepCopy()
	otherNamespace.Namespace = "cleanup-elsewhere"
	objects := []client.Object{healthy, expired, recent, pending, synced, previous, unversioned, deleting, predecessor, otherNamespace}
	r := newCleanupReconciler(t, interceptor.Funcs{}, objects...)
	if err := r.reconcileCleanup(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
	for _, object := range objects {
		err := r.Get(t.Context(), client.ObjectKeyFromObject(object), &runtimev1alpha1.Workload{})
		if object == expired || object == previous {
			if !apierrors.IsNotFound(err) {
				t.Errorf("obsolete workload %s: Get = %v, want NotFound", object.GetName(), err)
			}
		} else if err != nil {
			t.Errorf("protected workload %s/%s: %v", object.GetNamespace(), object.GetName(), err)
		}
	}
	if err := r.reconcileCleanup(t.Context(), rs); err != nil {
		t.Fatalf("repeat cleanup: %v", err)
	}
}

func TestReplicaSetCleanupWaitsForScaling(t *testing.T) {
	for _, state := range []string{"terminating", "scaling up", "scaling down"} {
		t.Run(state, func(t *testing.T) {
			rs := cleanupReplicaSet()
			switch state {
			case "terminating":
				now := metav1.Now()
				rs.DeletionTimestamp = &now
			case "scaling up":
				rs.Status.SetConditions(condition.UnknownCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp, "Pending", "scaling"))
			case "scaling down":
				rs.Status.SetConditions(condition.UnknownCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleDown, "Pending", "scaling"))
			}
			r := newCleanupReconciler(t, interceptor.Funcs{
				List: func(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error {
					t.Fatal("cleanup queried workloads before scaling finished")
					return nil
				},
			})
			if err := r.reconcileCleanup(t.Context(), rs); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestReplicaSetCleanupPropagatesAPIErrors(t *testing.T) {
	for _, operation := range []string{"list previous", "delete previous", "list unhealthy", "delete unhealthy"} {
		t.Run(operation, func(t *testing.T) {
			rs := cleanupReplicaSet()
			w := failedCleanupWorkload(rs, "failed", time.Now().Add(-time.Hour))
			if operation == "delete previous" {
				w.Annotations[workloadReplicaSetGenerationAnnotation] = cleanupPreviousHash
			}
			want := errors.New("API unavailable")
			lists := 0
			r := newCleanupReconciler(t, interceptor.Funcs{
				List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
					lists++
					if (operation == "list previous" && lists == 1) || (operation == "list unhealthy" && lists == 2) {
						return want
					}
					return c.List(ctx, list, opts...)
				},
				Delete: func(context.Context, client.WithWatch, client.Object, ...client.DeleteOption) error {
					return want
				},
			}, w)
			if err := r.reconcileCleanup(t.Context(), rs); !errors.Is(err, want) {
				t.Fatalf("cleanup = %v, want %v", err, want)
			}
			if err := r.Get(t.Context(), client.ObjectKeyFromObject(w), &runtimev1alpha1.Workload{}); err != nil {
				t.Fatalf("failed write removed the workload: %v", err)
			}
		})
	}
}

func TestReplicaSetScaleDownSelectsCurrentOwnedWorkloads(t *testing.T) {
	for _, desired := range []*int32{nil, new(int32(0)), new(int32(1)), new(int32(5))} {
		name := "unset"
		if desired != nil {
			name = fmt.Sprint(*desired)
		}
		t.Run(name, func(t *testing.T) {
			rs := cleanupReplicaSet()
			rs.Spec.Replicas = desired
			objects := make([]client.Object, 0, 6)
			for i := range 3 {
				objects = append(objects, cleanupWorkload(rs, fmt.Sprintf("current-%d", i)))
			}
			previous := cleanupWorkload(rs, "previous")
			previous.Annotations[workloadReplicaSetGenerationAnnotation] = cleanupPreviousHash
			predecessor := cleanupWorkload(rs, "predecessor")
			predecessor.OwnerReferences[0].UID = "previous-owner"
			otherNamespace := cleanupWorkload(rs, "other-namespace")
			otherNamespace.Namespace = "cleanup-elsewhere"
			objects = append(objects, previous, predecessor, otherNamespace)
			r := newCleanupReconciler(t, interceptor.Funcs{}, objects...)
			for range 2 {
				if err := r.reconcileScaleDown(t.Context(), rs); err != nil {
					t.Fatal(err)
				}
			}
			remaining := &runtimev1alpha1.WorkloadList{}
			if err := r.List(t.Context(), remaining); err != nil {
				t.Fatal(err)
			}
			want := 0
			if desired != nil {
				want = min(3, int(*desired))
			}
			if len(remaining.Items) != want+3 {
				t.Fatalf("remaining workloads = %d, want %d current and 3 protected", len(remaining.Items), want)
			}
			for _, protected := range []client.Object{previous, predecessor, otherNamespace} {
				if err := r.Get(t.Context(), client.ObjectKeyFromObject(protected), &runtimev1alpha1.Workload{}); err != nil {
					t.Fatalf("scale-down removed a protected workload: %v", err)
				}
			}
		})
	}
}

func TestReplicaSetScaleDownPropagatesAPIErrors(t *testing.T) {
	for _, operation := range []string{"list", "delete"} {
		t.Run(operation, func(t *testing.T) {
			rs := cleanupReplicaSet()
			rs.Spec.Replicas = nil
			w := cleanupWorkload(rs, "current")
			want := errors.New("API unavailable")
			funcs := interceptor.Funcs{}
			if operation == "list" {
				funcs.List = func(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error { return want }
			} else {
				funcs.Delete = func(context.Context, client.WithWatch, client.Object, ...client.DeleteOption) error { return want }
			}
			r := newCleanupReconciler(t, funcs, w)
			if err := r.reconcileScaleDown(t.Context(), rs); !errors.Is(err, want) {
				t.Fatalf("scale-down = %v, want %v", err, want)
			}
			if err := r.Get(t.Context(), client.ObjectKeyFromObject(w), &runtimev1alpha1.Workload{}); err != nil {
				t.Fatalf("failed write removed the workload: %v", err)
			}
		})
	}
}
