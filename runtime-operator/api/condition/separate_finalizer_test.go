package condition

import (
	"context"
	"errors"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

func TestSeparateFinalizerLifecycle(t *testing.T) {
	const finalizer = "separate-finalizer"
	scheme := runtime.NewScheme()
	scheme.AddKnownTypes(corev1.SchemeGroupVersion, &conditionedResource{})
	obj := &conditionedResource{ObjectMeta: metav1.ObjectMeta{Name: testName, Namespace: testNamespace}}
	c := fake.NewClientBuilder().WithScheme(scheme).WithObjects(obj).WithStatusSubresource(obj).Build()
	r := NewConditionedReconciler(c, scheme, obj, time.Minute)
	cleanupErr := errors.New("cleanup unavailable")
	cleanupCalls, conditionCalls := 0, 0
	r.SetFinalizer(finalizer, func(context.Context, *conditionedResource) error {
		cleanupCalls++
		return cleanupErr
	})
	r.SetCondition(TypeReady, func(context.Context, *conditionedResource) error {
		conditionCalls++
		return nil
	})
	cleanup := r.SeparateFinalizer()
	req := reconcile.Request{NamespacedName: client.ObjectKeyFromObject(obj)}
	ctx := t.Context()

	// A live object in the cleanup queue must acquire neither a finalizer nor status.
	if _, err := cleanup.Reconcile(ctx, req); err != nil {
		t.Fatal(err)
	}
	if err := c.Get(ctx, req.NamespacedName, obj); err != nil {
		t.Fatal(err)
	}
	if len(obj.Finalizers) != 0 || cleanupCalls != 0 || conditionCalls != 0 {
		t.Fatal("cleanup reconciler changed a live object")
	}
	if _, err := r.Reconcile(ctx, req); err != nil {
		t.Fatal(err)
	}
	if err := c.Get(ctx, req.NamespacedName, obj); err != nil {
		t.Fatal(err)
	}
	if !controllerutil.ContainsFinalizer(obj, finalizer) || conditionCalls != 1 {
		t.Fatal("normal reconciliation did not install the finalizer and reconcile conditions")
	}
	if err := c.Delete(ctx, obj); err != nil {
		t.Fatal(err)
	}

	// Already-queued normal reconciles must not execute any deletion work.
	if result, err := r.Reconcile(ctx, req); err != nil || result != (reconcile.Result{}) {
		t.Fatalf("normal reconciliation of a deleting object: result=%+v, err=%v", result, err)
	}
	if cleanupCalls != 0 || conditionCalls != 1 {
		t.Fatal("normal reconciler ran cleanup or conditions on a deleting object")
	}
	if _, err := cleanup.Reconcile(ctx, req); !errors.Is(err, cleanupErr) {
		t.Fatalf("cleanup error lost: %v", err)
	}
	if err := c.Get(ctx, req.NamespacedName, obj); err != nil {
		t.Fatal(err)
	}
	if !controllerutil.ContainsFinalizer(obj, finalizer) {
		t.Fatal("failed cleanup removed the finalizer")
	}

	cleanupErr = nil
	if _, err := cleanup.Reconcile(ctx, req); err != nil {
		t.Fatal(err)
	}
	if err := c.Get(ctx, req.NamespacedName, obj); !apierrors.IsNotFound(err) {
		t.Fatalf("successful cleanup did not release the object: %v", err)
	}
	if _, err := cleanup.Reconcile(ctx, req); err != nil {
		t.Fatal(err)
	}
	if cleanupCalls != 2 || conditionCalls != 1 {
		t.Fatalf("unexpected callback counts: cleanup=%d, conditions=%d", cleanupCalls, conditionCalls)
	}
}
