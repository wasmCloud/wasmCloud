package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	testListOperation        = "list"
	missingPodCase           = "missing Pod"
	deletingFinalizedPodCase = "deleting with finalizer"
	cleanupHoldFinalizer     = "cleanup.example/hold"
)

func TestHostPodReconcileFinalizers(t *testing.T) {
	for _, state := range []string{"live without finalizer", "live with finalizer", "deleting without finalizer", deletingFinalizedPodCase, missingPodCase} {
		t.Run(state, func(t *testing.T) {
			pod := livePod("host-pod", "10.1.2.4")
			if state == "deleting without finalizer" || state == deletingFinalizedPodCase {
				pod = terminatingPod(pod.Status.PodIP, time.Now(), 0)
				pod.Finalizers = []string{cleanupHoldFinalizer}
			}
			if state == "live with finalizer" || state == deletingFinalizedPodCase {
				pod.Finalizers = append(pod.Finalizers, podHostFinalizerName)
			}
			patches := 0
			c := hostPodClientBuilder(t, pod).WithInterceptorFuncs(interceptor.Funcs{
				Patch: func(ctx context.Context, c client.WithWatch, obj client.Object, patch client.Patch, opts ...client.PatchOption) error {
					patches++
					return c.Patch(ctx, obj, patch, opts...)
				},
			}).Build()
			key := client.ObjectKeyFromObject(pod)
			if state == missingPodCase {
				key.Name = "absent"
			}
			r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace, RetiredHosts: &RetiredHosts{}}
			for range 2 {
				if result, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil || result != (ctrl.Result{}) {
					t.Fatalf("Reconcile = %+v, %v", result, err)
				}
			}
			wantPatches := 0
			if state == "live without finalizer" || state == deletingFinalizedPodCase {
				wantPatches = 1
			}
			if patches != wantPatches {
				t.Fatalf("patches = %d, want %d", patches, wantPatches)
			}
			got := &corev1.Pod{}
			if err := c.Get(t.Context(), client.ObjectKeyFromObject(pod), got); err != nil {
				t.Fatal(err)
			}
			wantFinalizer := pod.DeletionTimestamp.IsZero() && state != missingPodCase
			found := false
			for _, f := range got.Finalizers {
				found = found || f == podHostFinalizerName
			}
			if found != wantFinalizer {
				t.Fatalf("finalizers = %v, want host finalizer %v", got.Finalizers, wantFinalizer)
			}
		})
	}
}

func TestHostPodCleanupFailureRetainsFinalizerUntilRetry(t *testing.T) {
	for _, operation := range []string{"get", testListOperation, daemonSetDeleteOperation, "patch"} {
		t.Run(operation, func(t *testing.T) {
			now := time.Now()
			pod := terminatingPod("10.1.2.3", now, 0)
			pod.Finalizers = []string{podHostFinalizerName, cleanupHoldFinalizer}
			host := hostAt("old-host", pod.Status.PodIP, now.Add(-time.Hour))
			host.HostID = testHeartbeatHostID
			cause := errors.New("API unavailable")
			fail := true
			c := hostPodClientBuilder(t, pod, host).WithInterceptorFuncs(interceptor.Funcs{
				Get: func(ctx context.Context, c client.WithWatch, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
					if fail && operation == "get" {
						return cause
					}
					return c.Get(ctx, key, obj, opts...)
				},
				List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
					if fail && operation == testListOperation {
						return cause
					}
					return c.List(ctx, list, opts...)
				},
				Delete: func(ctx context.Context, c client.WithWatch, obj client.Object, opts ...client.DeleteOption) error {
					if fail && operation == daemonSetDeleteOperation {
						return cause
					}
					return c.Delete(ctx, obj, opts...)
				},
				Patch: func(ctx context.Context, c client.WithWatch, obj client.Object, patch client.Patch, opts ...client.PatchOption) error {
					if fail && operation == "patch" {
						return cause
					}
					return c.Patch(ctx, obj, patch, opts...)
				},
			}).Build()
			r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace, RetiredHosts: &RetiredHosts{}}
			req := ctrl.Request{NamespacedName: client.ObjectKeyFromObject(pod)}
			if _, err := r.Reconcile(t.Context(), req); !errors.Is(err, cause) {
				t.Fatalf("Reconcile = %v, want API error", err)
			}
			fail = false
			got := &corev1.Pod{}
			if err := c.Get(t.Context(), req.NamespacedName, got); err != nil {
				t.Fatal(err)
			}
			if len(got.Finalizers) != 2 {
				t.Fatalf("failed cleanup removed finalizer: %v", got.Finalizers)
			}
			for range 2 {
				if _, err := r.Reconcile(t.Context(), req); err != nil {
					t.Fatal(err)
				}
			}
			if err := c.Get(t.Context(), req.NamespacedName, got); err != nil {
				t.Fatal(err)
			}
			if len(got.Finalizers) != 1 || got.Finalizers[0] != cleanupHoldFinalizer {
				t.Fatalf("finalizers after retry = %v", got.Finalizers)
			}
			if err := c.Get(t.Context(), client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
				t.Fatalf("host after cleanup: %v", err)
			}
			if !r.RetiredHosts.contains(host.HostID) {
				t.Fatal("deleted host was not retired")
			}
		})
	}
}
