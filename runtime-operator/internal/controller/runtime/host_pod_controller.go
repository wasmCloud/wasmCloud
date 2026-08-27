// +kubebuilder:rbac:groups="",resources=pods,verbs=get;list;patch;watch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts,verbs=get;list;delete

package runtime

import (
	"context"
	"time"

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
//
// Pod side may be cross-namespace: host Pods can run in any namespace listed
// in operator.hostNamespaces. Host side is single-namespace: every Host CRD
// lives in the operator's own namespace.
type HostPodReconciler struct {
	client.Client
	Scheme *runtime.Scheme
	// OperatorNamespace is the namespace where Host CRDs live.
	OperatorNamespace string
}

// Reconcile is called whenever a Pod with HostPodLabel changes.
// RBAC markers for this controller are at the top of the file — see header comment.
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

	if err := r.deleteHostForPod(ctx, pod); err != nil {
		return ctrl.Result{}, err
	}

	base := pod.DeepCopy()
	controllerutil.RemoveFinalizer(pod, podHostFinalizerName)
	return ctrl.Result{}, r.Patch(ctx, pod, client.MergeFrom(base))
}

// deleteHostForPod deletes the Host CRD registered by the given terminating
// Pod, identified by matching Host.Hostname to the Pod's IP. The list is always
// scoped to the operator's own namespace — that is the one and only place Host
// objects live, regardless of where the underlying host pod runs. Uses a field
// index so it does not scan every Host.
//
// Kubernetes recycles Pod IPs, and this controller's finalizer holds the
// terminating Pod in the API long after kubelet released its IP, so a
// replacement Pod may already hold that IP and have registered a Host under it.
// A Host created at or after this Pod was condemned is the replacement's, and
// deleting it would take a live host's Workloads with it.
func (r *HostPodReconciler) deleteHostForPod(ctx context.Context, pod *corev1.Pod) error {
	podIP := pod.Status.PodIP
	if podIP == "" {
		return nil
	}

	var hosts runtimev1alpha1.HostList
	if err := r.List(ctx, &hosts,
		client.InNamespace(r.OperatorNamespace),
		client.MatchingFields{hostnameFieldIndex: podIP},
	); err != nil {
		return err
	}

	log := ctrl.LoggerFrom(ctx)
	condemnedAt := podCondemnedAt(pod)
	for i := range hosts.Items {
		host := &hosts.Items[i]
		if !host.CreationTimestamp.Time.Before(condemnedAt) {
			log.Info("keeping Host registered at or after this pod was condemned; its pod IP was recycled",
				"host", host.Name, "hostID", host.HostID, "podIP", podIP,
				"hostCreated", host.CreationTimestamp.Time, "podCondemnedAt", condemnedAt)
			continue
		}
		if err := r.Delete(ctx, host); client.IgnoreNotFound(err) != nil {
			return err
		}
	}
	return nil
}

// podCondemnedAt returns the instant the Pod was told to go away, the earliest
// its IP could have been released to another Pod.
//
// DeletionTimestamp is the grace deadline, request time +
// DeletionGracePeriodSeconds, so on the Kubernetes default it sits 30s in the
// future and covers every Host a replacement registered during the grace window.
//
// Both timestamps carry one-second resolution, so a Host created in the same
// second is kept — erring toward an orphan the unreachable-host path reaps a
// window later, over deleting a live host's Workloads.
// A Pod that is not terminating returns the zero time, which spares every Host.
func podCondemnedAt(pod *corev1.Pod) time.Time {
	if pod.DeletionTimestamp == nil {
		return time.Time{}
	}
	condemned := pod.DeletionTimestamp.Time
	if grace := pod.DeletionGracePeriodSeconds; grace != nil {
		condemned = condemned.Add(-time.Duration(*grace) * time.Second)
	}
	return condemned
}

// SetupWithManager registers the controller and the Host field index it depends on.
func (r *HostPodReconciler) SetupWithManager(mgr ctrl.Manager) error {
	// Index Host objects by Hostname (= pod IP) so deleteHostForPod can do a
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
