package runtime

import (
	"context"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/predicate"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	// HostPodLabel is a marker label that must be present on host Pods.
	// No specific value is required — its presence is enough to opt the Pod
	// into lifecycle tracking. The operator maps the Pod to its Host CRD via
	// pod.Status.PodIP, which matches Host.Hostname (set from req.Hostname in
	// the NATS heartbeat). Using PodIP means the HostGroup deployment template
	// does not need to know the wasmCloud host ID ahead of time.
	HostPodLabel = "wasmcloud.com/hostgroup"

	podHostFinalizerName = "runtime.wasmcloud.dev/pod-host-finalizer"

	// hostnameFieldIndex is the field indexer key for Host.Hostname,
	// enabling O(1) lookup of a Host by its pod IP without listing all hosts.
	hostnameFieldIndex = ".hostname"
)

// HostPodReconciler bridges Pod lifecycle to Host CRD lifecycle.
//
// It adds a finalizer to every Pod labeled with HostPodLabel. When a Pod is
// deleted (DeletionTimestamp set), the finalizer fires and the corresponding
// Host CRD — identified by matching Host.Hostname to pod.Status.PodIP — is
// deleted before Kubernetes removes the Pod from etcd.
//
// Deleting the Host CRD triggers the HostReconciler finalizer, which in turn
// deletes all Workload objects assigned to that host. This replaces the
// previous path where the operator waited for up to UnreachableTimeout +
// hostReconcileInterval (≈2 min) to discover a dead host via missed heartbeats.
type HostPodReconciler struct {
	client.Client
	Scheme    *runtime.Scheme
	Namespace string
}

// Reconcile is called whenever a Pod with HostPodLabel changes.
//
// +kubebuilder:rbac:groups="",resources=pods,verbs=get;list;patch;watch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts,verbs=get;list;delete
func (r *HostPodReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	pod := &corev1.Pod{}
	if err := r.Get(ctx, req.NamespacedName, pod); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	if pod.DeletionTimestamp.IsZero() {
		// Pod is alive — ensure our finalizer is present.
		if !controllerutil.ContainsFinalizer(pod, podHostFinalizerName) {
			base := pod.DeepCopy()
			controllerutil.AddFinalizer(pod, podHostFinalizerName)
			return ctrl.Result{}, r.Patch(ctx, pod, client.MergeFrom(base))
		}
		return ctrl.Result{}, nil
	}

	// Pod is being deleted — run cleanup only if our finalizer is still present.
	if !controllerutil.ContainsFinalizer(pod, podHostFinalizerName) {
		return ctrl.Result{}, nil
	}

	if podIP := pod.Status.PodIP; podIP != "" {
		if err := r.deleteHostForIP(ctx, podIP); err != nil {
			return ctrl.Result{}, err
		}
	}

	base := pod.DeepCopy()
	controllerutil.RemoveFinalizer(pod, podHostFinalizerName)
	return ctrl.Result{}, r.Patch(ctx, pod, client.MergeFrom(base))
}

// deleteHostForIP deletes the Host CRD whose Hostname matches podIP.
// Uses a field index so it does not scan all Host objects.
func (r *HostPodReconciler) deleteHostForIP(ctx context.Context, podIP string) error {
	var hosts runtimev1alpha1.HostList
	if err := r.List(ctx, &hosts, client.MatchingFields{hostnameFieldIndex: podIP}); err != nil {
		return err
	}
	for i := range hosts.Items {
		if err := r.Delete(ctx, &hosts.Items[i]); client.IgnoreNotFound(err) != nil {
			return err
		}
	}
	return nil
}

// SetupWithManager registers the controller and the Host field index it depends on.
func (r *HostPodReconciler) SetupWithManager(mgr ctrl.Manager) error {
	// Index Host objects by Hostname (= pod IP) so deleteHostForIP can do a
	// direct lookup rather than listing every Host and filtering in memory.
	if err := mgr.GetFieldIndexer().IndexField(
		context.Background(),
		&runtimev1alpha1.Host{},
		hostnameFieldIndex,
		func(obj client.Object) []string {
			host, ok := obj.(*runtimev1alpha1.Host)
			if !ok || host.Hostname == "" {
				return nil
			}
			return []string{host.Hostname}
		},
	); err != nil {
		return err
	}

	return ctrl.NewControllerManagedBy(mgr).
		For(&corev1.Pod{}, builder.WithPredicates(
			// Only enqueue Pods that carry the HostPodLabel — avoids processing
			// every Pod in the namespace. Namespace scoping is handled by the
			// cache (ByObject in cmd/main.go), so no namespace check is needed here.
			predicate.NewPredicateFuncs(func(obj client.Object) bool {
				_, ok := obj.GetLabels()[HostPodLabel]
				return ok
			}),
		)).
		Named("host-pod").
		Complete(r)
}
