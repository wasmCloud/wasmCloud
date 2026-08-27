package runtime

import (
	"context"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// newHostPodClient builds a fake client wired with the same Hostname index
// deleteHostForPod relies on in production, seeded with the given objects.
func newHostPodClient(t *testing.T, objs ...client.Object) client.Client {
	t.Helper()
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	if err := corev1.AddToScheme(s); err != nil {
		t.Fatalf("add corev1: %v", err)
	}
	return fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.Host{}, hostnameFieldIndex,
			func(obj client.Object) []string {
				host, ok := obj.(*runtimev1alpha1.Host)
				if !ok || host.Hostname == "" {
					return nil
				}
				return []string{host.Hostname}
			}).
		Build()
}

func hostAt(name, hostname string, created time.Time) *runtimev1alpha1.Host {
	return &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{
			Name:              name,
			Namespace:         testNamespace,
			CreationTimestamp: metav1.NewTime(created),
		},
		HostID:   name + "-id",
		Hostname: hostname,
	}
}

// terminatingPod builds a Pod condemned at condemnedAt with the given grace
// period, mirroring how the API server records a delete: DeletionTimestamp is
// the grace *deadline*, condemnedAt + grace, not the moment of the request.
func terminatingPod(podIP string, condemnedAt time.Time, grace int64) *corev1.Pod {
	deadline := metav1.NewTime(condemnedAt.Add(time.Duration(grace) * time.Second))
	return &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name:                       "hostgroup-a",
			Namespace:                  testNamespace,
			Labels:                     map[string]string{HostPodLabel: "pool-a"},
			DeletionTimestamp:          &deadline,
			DeletionGracePeriodSeconds: &grace,
			Finalizers:                 []string{podHostFinalizerName},
		},
		Status: corev1.PodStatus{PodIP: podIP},
	}
}

// TestDeleteHostForPod covers the Pod-IP-to-Host mapping the finalizer uses.
// Kubernetes recycles Pod IPs, and this controller's own finalizer holds the
// terminating Pod object in the API well past the point where its IP was
// released — so the Host registered under that IP may already belong to the
// replacement Pod, and deleting it would cascade into a live host's Workloads.
func TestDeleteHostForPod(t *testing.T) {
	const podIP = "10.1.2.3"
	condemnedAt := time.Now()

	t.Run("deletes the host this pod registered", func(t *testing.T) {
		host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, host)
		r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
			t.Errorf("the pod's own host should have been deleted, got err=%v", err)
		}
	})

	t.Run("spares a host registered after this pod was deleted", func(t *testing.T) {
		recycled := hostAt("replacement-host", podIP, condemnedAt.Add(2*time.Second))
		c := newHostPodClient(t, recycled)
		r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(recycled), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("a host that registered after the pod was deleted must be left alone, got err=%v", err)
		}
	})

	t.Run("spares a replacement registered inside the grace window", func(t *testing.T) {
		// The chart pins terminationGracePeriodSeconds to 0, but the
		// Kubernetes default is 30s and a hand-written manifest or a drain
		// gets one. DeletionTimestamp then sits in the future, covering the
		// live hosts a replacement registered while this Pod wound down.
		recycled := hostAt("replacement-host", podIP, condemnedAt.Add(8*time.Second))
		c := newHostPodClient(t, recycled)
		r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 30)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(recycled), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("a host registered during the grace window belongs to the replacement pod, got err=%v", err)
		}
	})

	t.Run("still deletes its own host when a grace period is set", func(t *testing.T) {
		host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, host)
		r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 30)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
			t.Errorf("the pod's own host should still be deleted, got err=%v", err)
		}
	})

	t.Run("leaves hosts on other IPs alone", func(t *testing.T) {
		other := hostAt("other-host", "10.1.2.4", condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, other)
		r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(other), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("host on another IP must be untouched, got err=%v", err)
		}
	})

	t.Run("pod without an IP deletes nothing", func(t *testing.T) {
		host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, host)
		r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace}

		pod := terminatingPod("", condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("a pod with no IP must not delete any host, got err=%v", err)
		}
	})
}

// TestHostPodReconcile_RemovesFinalizerAfterCleanup walks the terminating-pod
// path end to end: the host is deleted and the finalizer released so
// Kubernetes can finish removing the Pod.
func TestHostPodReconcile_RemovesFinalizerAfterCleanup(t *testing.T) {
	const podIP = "10.1.2.3"
	condemnedAt := time.Now()

	host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
	pod := terminatingPod(podIP, condemnedAt, 0)
	c := newHostPodClient(t, host, pod)
	r := &HostPodReconciler{Client: c, OperatorNamespace: testNamespace}

	ctx := context.Background()
	req := ctrl.Request{NamespacedName: client.ObjectKeyFromObject(pod)}
	if _, err := r.Reconcile(ctx, req); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}

	if err := c.Get(ctx, client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
		t.Errorf("host should have been deleted, got err=%v", err)
	}
	// Releasing the last finalizer lets the fake client complete the deletion.
	if err := c.Get(ctx, client.ObjectKeyFromObject(pod), &corev1.Pod{}); !apierrors.IsNotFound(err) {
		t.Errorf("pod finalizer should have been removed, got err=%v", err)
	}
}
