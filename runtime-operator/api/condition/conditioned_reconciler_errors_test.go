package condition

import (
	"context"
	"encoding/json"
	"errors"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

func conditionedScheme(t *testing.T) *runtime.Scheme {
	t.Helper()
	scheme := runtime.NewScheme()
	if err := corev1.AddToScheme(scheme); err != nil {
		t.Fatalf("failed to add corev1 to scheme: %v", err)
	}
	scheme.AddKnownTypes(corev1.SchemeGroupVersion, &conditionedResource{})
	return scheme
}

// A finalizer that ran but could not be removed must surface the error and
// leave the finalizer in place, so the next pass finalizes again rather than
// letting the object go with its cleanup unrecorded.
func TestHandleFinalizerRemovalPatchFailure(t *testing.T) {
	const finalizer = "test-finalizer"
	now := metav1.Now()
	obj := &conditionedResource{ObjectMeta: metav1.ObjectMeta{
		Namespace: testNamespace, Name: testName, Finalizers: []string{finalizer}, DeletionTimestamp: &now,
	}}
	boom := errors.New("boom")
	kubeClient := fake.NewClientBuilder().
		WithScheme(conditionedScheme(t)).
		WithObjects(obj).
		WithInterceptorFuncs(interceptor.Funcs{
			Patch: func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
				return boom
			},
		}).
		Build()

	finalized := 0
	updated, err := HandleFinalizer(context.Background(), kubeClient, obj, finalizer,
		func(context.Context, *conditionedResource) error {
			finalized++
			return nil
		})
	if !errors.Is(err, boom) {
		t.Fatalf("expected the patch error, got %v", err)
	}
	if updated {
		t.Error("a failed finalizer removal reported the object as updated")
	}
	if finalized != 1 {
		t.Errorf("finalizer callback ran %d times, want 1", finalized)
	}
	stored := &conditionedResource{}
	if err := kubeClient.Get(context.Background(), client.ObjectKeyFromObject(obj), stored); err != nil {
		t.Fatalf("get: %v", err)
	}
	if len(stored.Finalizers) != 1 || stored.Finalizers[0] != finalizer {
		t.Errorf("finalizer should remain after a failed removal, got %v", stored.Finalizers)
	}
}

// Condition functions can be called outside a reconcile pass (in tests, or by
// helpers shared with other code). The context helpers must not panic there.
func TestReconcilerContextHelpersOutsideAPass(t *testing.T) {
	ctx := context.Background()
	if got := GetReconcilerContext(ctx); got == nil || *got != (ReconcilerContext{}) {
		t.Fatalf("expected an empty placeholder context, got %+v", got)
	}
	ForceStatusUpdate(ctx)
	ForceRequeue(ctx)
	RequireOptimisticLock(ctx)
	// Each call gets its own placeholder, so nothing leaks between callers.
	if got := GetReconcilerContext(ctx); *got != (ReconcilerContext{}) {
		t.Errorf("placeholder context kept state between calls: %+v", got)
	}
}

// Only NotFound is swallowed when reading the object; any other read failure
// is returned so the request is retried with backoff.
func TestConditionedReconcilerGetFailure(t *testing.T) {
	scheme := conditionedScheme(t)
	obj := &conditionedResource{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: testName}}
	unavailable := apierrors.NewServiceUnavailable("apiserver down")
	kubeClient := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(obj).
		WithInterceptorFuncs(interceptor.Funcs{
			Get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
				return unavailable
			},
		}).
		Build()

	r := NewConditionedReconciler(kubeClient, scheme, obj, time.Second)
	called := false
	r.SetCondition(testCondition, func(context.Context, *conditionedResource) error {
		called = true
		return nil
	})
	_, err := r.Reconcile(context.Background(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(obj)})
	if !apierrors.IsServiceUnavailable(err) {
		t.Fatalf("expected the read error, got %v", err)
	}
	if called {
		t.Error("conditions ran without a successful read")
	}
}

// When the fresh read taken to retry a conflicting status patch itself fails,
// the pass fails with that error instead of retrying blind.
func TestConditionedReconcilerRetryStatusConflictsReadFailure(t *testing.T) {
	scheme := conditionedScheme(t)
	obj := &conditionedResource{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: testName}}
	gr := schema.GroupResource{Resource: "conditionedresources"}
	kubeClient := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(obj).
		WithStatusSubresource(obj).
		WithInterceptorFuncs(interceptor.Funcs{
			SubResourcePatch: func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
				return apierrors.NewConflict(gr, testName, errors.New("stale"))
			},
		}).
		Build()
	unavailable := apierrors.NewServiceUnavailable("apiserver down")
	reader := fake.NewClientBuilder().
		WithScheme(scheme).
		WithInterceptorFuncs(interceptor.Funcs{
			Get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
				return unavailable
			},
		}).
		Build()

	r := NewConditionedReconciler(kubeClient, scheme, obj, time.Second)
	r.RetryStatusConflicts(reader, nil)
	r.SetCondition(testCondition, func(context.Context, *conditionedResource) error { return nil })
	hooked := false
	r.AddPostHook(func(context.Context, *conditionedResource) error {
		hooked = true
		return nil
	})

	_, err := r.Reconcile(context.Background(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(obj)})
	if !apierrors.IsServiceUnavailable(err) {
		t.Fatalf("expected the re-read error, got %v", err)
	}
	if hooked {
		t.Error("post-hooks ran after the status write failed")
	}
}

// reapplyStatus sends nothing when the pass changed nothing. Otherwise its
// patch keeps the pass's metadata change and is pinned to the fresh read.
func TestReapplyStatus(t *testing.T) {
	scheme := conditionedScheme(t)
	key := client.ObjectKey{Namespace: testNamespace, Name: testName}

	type sentPatches struct {
		count int
		last  map[string]any
	}
	newReconciler := func(t *testing.T) (*ConditionedReconciler[*conditionedResource], client.Client, *sentPatches, *conditionedResource) {
		t.Helper()
		obj := &conditionedResource{ObjectMeta: metav1.ObjectMeta{Namespace: key.Namespace, Name: key.Name, UID: "uid"}}
		patches := &sentPatches{}
		kubeClient := fake.NewClientBuilder().
			WithScheme(scheme).
			WithObjects(obj).
			WithStatusSubresource(obj).
			WithInterceptorFuncs(interceptor.Funcs{
				SubResourcePatch: func(ctx context.Context, c client.Client, sub string, o client.Object, p client.Patch, opts ...client.SubResourcePatchOption) error {
					patches.count++
					data, err := p.Data(o)
					if err != nil {
						return err
					}
					patches.last = map[string]any{}
					if err := json.Unmarshal(data, &patches.last); err != nil {
						return err
					}
					return c.SubResource(sub).Patch(ctx, o, p, opts...)
				},
			}).
			Build()
		original := &conditionedResource{}
		if err := kubeClient.Get(context.Background(), key, original); err != nil {
			t.Fatalf("get: %v", err)
		}
		r := NewConditionedReconciler(kubeClient, scheme, original, time.Second)
		r.RetryStatusConflicts(kubeClient, nil)
		return r, kubeClient, patches, original
	}

	t.Run("no changes", func(t *testing.T) {
		r, _, patches, original := newReconciler(t)
		if err := r.reapplyStatus(context.Background(), original, original.DeepCopy()); err != nil {
			t.Fatalf("reapply: %v", err)
		}
		if patches.count != 0 {
			t.Errorf("an empty diff sent %d patches", patches.count)
		}
	})

	t.Run("metadata and status", func(t *testing.T) {
		r, kubeClient, patches, original := newReconciler(t)
		desired := original.DeepCopy()
		desired.Labels = map[string]string{"pass": "seen"}
		desired.Status.SetConditions(ReadyCondition(testCondition))

		// Another writer moves the object on after the pass read it.
		other := original.DeepCopy()
		other.Status.SetConditions(ReadyCondition("Other"))
		if err := kubeClient.Status().Update(context.Background(), other); err != nil {
			t.Fatalf("concurrent update: %v", err)
		}

		if err := r.reapplyStatus(context.Background(), original, desired); err != nil {
			t.Fatalf("reapply: %v", err)
		}
		if patches.count != 1 {
			t.Fatalf("sent %d patches, want 1", patches.count)
		}
		// The patch keeps the pass's metadata change and is pinned to the
		// fresh read, so a later writer makes it conflict instead of being
		// overwritten.
		metadata, _ := patches.last["metadata"].(map[string]any)
		if labels, _ := metadata["labels"].(map[string]any); labels["pass"] != "seen" {
			t.Errorf("the patch dropped the pass's label change: %v", patches.last)
		}
		if metadata["resourceVersion"] != other.ResourceVersion {
			t.Errorf("patch resourceVersion = %v, want the fresh read's %s", metadata["resourceVersion"], other.ResourceVersion)
		}
		stored := &conditionedResource{}
		if err := kubeClient.Get(context.Background(), key, stored); err != nil {
			t.Fatalf("get: %v", err)
		}
		if stored.Status.GetCondition(testCondition).Status != ConditionTrue {
			t.Errorf("the pass's condition was not written: %+v", stored.Status.Conditions)
		}
	})
}
