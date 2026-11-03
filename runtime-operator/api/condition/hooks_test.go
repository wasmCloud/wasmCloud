package condition

import (
	"context"
	"errors"
	"reflect"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

func TestReconcileHookOrderingAndFailures(t *testing.T) {
	const (
		firstPreCall   = "pre-1"
		secondPreCall  = "pre-2"
		conditionCall  = "condition"
		firstPostCall  = "post-1"
		secondPostCall = "post-2"
	)
	wantError := errors.New("hook failed")
	for _, tt := range []struct {
		name      string
		preError  error
		postError error
		wantCalls []string
		wantError error
	}{
		{name: "success", wantCalls: []string{firstPreCall, secondPreCall, conditionCall, firstPostCall, secondPostCall}},
		{name: "pre failure", preError: wantError, wantError: wantError, wantCalls: []string{firstPreCall}},
		{name: "pre skip", preError: ErrSkipReconciliation(), wantCalls: []string{firstPreCall}},
		{name: "post failure", postError: wantError, wantError: wantError, wantCalls: []string{firstPreCall, secondPreCall, conditionCall, firstPostCall}},
		{name: "post skip", postError: ErrSkipReconciliation(), wantCalls: []string{firstPreCall, secondPreCall, conditionCall, firstPostCall}},
	} {
		t.Run(tt.name, func(t *testing.T) {
			scheme := runtime.NewScheme()
			scheme.AddKnownTypes(corev1.SchemeGroupVersion, &conditionedResource{})
			obj := &conditionedResource{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: testName}}
			c := fake.NewClientBuilder().WithScheme(scheme).WithObjects(obj).Build()
			r := NewConditionedReconciler(c, scheme, obj, time.Minute)
			var calls []string
			r.AddPreHook(func(context.Context, *conditionedResource) error {
				calls = append(calls, firstPreCall)
				return tt.preError
			})
			r.AddPreHook(func(context.Context, *conditionedResource) error { calls = append(calls, secondPreCall); return nil })
			r.SetCondition(testCondition, func(context.Context, *conditionedResource) error {
				calls = append(calls, conditionCall)
				return ErrNoop()
			})
			r.AddPostHook(func(context.Context, *conditionedResource) error {
				calls = append(calls, firstPostCall)
				return tt.postError
			})
			r.AddPostHook(func(context.Context, *conditionedResource) error { calls = append(calls, secondPostCall); return nil })
			result, err := r.Reconcile(t.Context(), reconcile.Request{NamespacedName: client.ObjectKeyFromObject(obj)})
			if !errors.Is(err, tt.wantError) || !reflect.DeepEqual(calls, tt.wantCalls) {
				t.Fatalf("Reconcile(): calls=%v error=%v, want calls=%v error=%v", calls, err, tt.wantCalls, tt.wantError)
			}
			if err == nil && result.RequeueAfter != time.Minute {
				t.Fatalf("requeue = %s, want one minute", result.RequeueAfter)
			}
		})
	}
}
