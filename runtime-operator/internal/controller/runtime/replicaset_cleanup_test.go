package runtime

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"testing"
	"testing/synctest"
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
	synctest.Test(t, testCleanupPreservesProtectedWorkloads)
}

func testCleanupPreservesProtectedWorkloads(t *testing.T) {
	rs := cleanupReplicaSet()
	now := time.Now()
	expiredAt := now.Add(-time.Hour)
	healthy := failedCleanupWorkload(rs, "healthy", expiredAt)
	healthy.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	expired := failedCleanupWorkload(rs, "expired", expiredAt)
	recent := failedCleanupWorkload(rs, "recent", now)
	pending := cleanupWorkload(rs, "pending")
	synced := failedCleanupWorkload(rs, "synced", expiredAt)
	synced.Status.Conditions[0].Status = condition.ConditionTrue
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
	previousPredecessor := previous.DeepCopy()
	previousPredecessor.Name = "previous-predecessor"
	previousPredecessor.OwnerReferences[0].UID = predecessor.OwnerReferences[0].UID
	previousOtherNamespace := previous.DeepCopy()
	previousOtherNamespace.Namespace = otherNamespace.Namespace
	objects := []client.Object{
		healthy, expired, recent, pending, synced, previous, unversioned, deleting,
		predecessor, otherNamespace, previousPredecessor, previousOtherNamespace,
	}
	deletes := make(map[client.ObjectKey]int)
	r := newCleanupReconciler(t, interceptor.Funcs{
		Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
			deletes[client.ObjectKeyFromObject(obj)]++
			return c.Delete(ctx, obj, opts...)
		},
	}, objects...)
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
	wantDeletes := map[client.ObjectKey]int{
		client.ObjectKeyFromObject(expired):  1,
		client.ObjectKeyFromObject(previous): 1,
	}
	if !reflect.DeepEqual(deletes, wantDeletes) {
		t.Fatalf("delete attempts = %v, want %v", deletes, wantDeletes)
	}
}

func TestReplicaSetCleanupGracePeriod(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		rs := cleanupReplicaSet()
		w := failedCleanupWorkload(rs, "recovering", time.Now())
		r := newCleanupReconciler(t, interceptor.Funcs{}, w)
		for _, advance := range []time.Duration{0, workloadReplicaSetReplicaGracePeriod - time.Nanosecond} {
			time.Sleep(advance)
			if err := r.reconcileCleanup(t.Context(), rs); err != nil {
				t.Fatal(err)
			}
			if err := r.Get(t.Context(), client.ObjectKeyFromObject(w), &runtimev1alpha1.Workload{}); err != nil {
				t.Fatalf("workload removed before the grace period expired: %v", err)
			}
		}
		time.Sleep(time.Nanosecond)
		if err := r.reconcileCleanup(t.Context(), rs); err != nil {
			t.Fatal(err)
		}
		if err := r.Get(t.Context(), client.ObjectKeyFromObject(w), &runtimev1alpha1.Workload{}); !apierrors.IsNotFound(err) {
			t.Fatalf("expired workload: Get = %v, want NotFound", err)
		}
	})
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
	previousKey := client.ObjectKey{Namespace: testNamespace, Name: "previous-failure"}
	currentKey := client.ObjectKey{Namespace: testNamespace, Name: "current-failure"}
	for _, tc := range []struct {
		operation   string
		failList    int
		failDelete  client.ObjectKey
		wantLists   int
		wantDeletes []client.ObjectKey
	}{
		{"list previous", 1, client.ObjectKey{}, 1, nil},
		{"delete previous", 0, previousKey, 1, []client.ObjectKey{previousKey}},
		{"list unhealthy", 2, client.ObjectKey{}, 2, []client.ObjectKey{previousKey}},
		{"delete unhealthy", 0, currentKey, 2, []client.ObjectKey{previousKey, currentKey}},
	} {
		t.Run(tc.operation, func(t *testing.T) {
			rs := cleanupReplicaSet()
			previous := cleanupWorkload(rs, previousKey.Name)
			previous.Annotations[workloadReplicaSetGenerationAnnotation] = cleanupPreviousHash
			current := failedCleanupWorkload(rs, currentKey.Name, time.Now().Add(-time.Hour))
			want := errors.New("API unavailable")
			lists := 0
			var deletes []client.ObjectKey
			injected := false
			r := newCleanupReconciler(t, interceptor.Funcs{
				List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
					lists++
					if !injected && lists == tc.failList {
						injected = true
						return want
					}
					return c.List(ctx, list, opts...)
				},
				Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
					key := client.ObjectKeyFromObject(obj)
					deletes = append(deletes, key)
					if !injected && key == tc.failDelete {
						injected = true
						return want
					}
					return c.Delete(ctx, obj, opts...)
				},
			}, previous, current)
			if err := r.reconcileCleanup(t.Context(), rs); !errors.Is(err, want) {
				t.Fatalf("cleanup = %v, want %v", err, want)
			}
			if !injected || lists != tc.wantLists || !reflect.DeepEqual(deletes, tc.wantDeletes) {
				t.Fatalf("cleanup calls: injected=%t lists=%d deletes=%v; want lists=%d deletes=%v", injected, lists, deletes, tc.wantLists, tc.wantDeletes)
			}
			for _, key := range []client.ObjectKey{previousKey, currentKey} {
				err := r.Get(t.Context(), key, &runtimev1alpha1.Workload{})
				previousRemoved := key == previousKey && tc.wantLists == 2
				if previousRemoved {
					if !apierrors.IsNotFound(err) {
						t.Fatalf("previous revision: Get = %v, want NotFound", err)
					}
				} else if err != nil {
					t.Fatalf("failed operation removed %s: %v", key, err)
				}
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
