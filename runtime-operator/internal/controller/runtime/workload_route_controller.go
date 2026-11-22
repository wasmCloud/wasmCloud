// +kubebuilder:rbac:groups=discovery.k8s.io,resources=endpointslices,verbs=create;delete;get;list;patch;update;watch
// +kubebuilder:rbac:groups="",resources=services,verbs=get;list;watch
// +kubebuilder:rbac:groups="",resources=services/finalizers,verbs=update
// +kubebuilder:rbac:groups="",resources=pods,verbs=get;list;watch

package runtime

import (
	"context"
	"fmt"
	"hash/fnv"
	"net"
	"reflect"
	"sort"

	corev1 "k8s.io/api/core/v1"
	discoveryv1 "k8s.io/api/discovery/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	// workloadKubernetesServiceIndex indexes Workloads by their Kubernetes.Service name.
	workloadKubernetesServiceIndex = "spec.kubernetes.service.name"
	// hostIDIndex indexes Hosts by their HostID field for O(1) lookup.
	hostIDIndex = "spec.hostId"
	// routeManagerLabel is the label applied to EndpointSlices managed by this controller.
	routeManagerLabel = "wasmcloud.dev/route-manager"
	// routeManagedValue is the routeManagerLabel value that marks a slice as managed.
	routeManagedValue = "true"
)

// WorkloadRouteReconciler maintains EndpointSlices for Kubernetes Services
// referenced by WorkloadDeployments. When a Workload becomes ready, this
// reconciler ensures the Service's EndpointSlice contains the pod IP of the
// host running that workload.
type WorkloadRouteReconciler struct {
	client.Client
	Scheme *runtime.Scheme
	// OperatorNamespace is where every Host CRD lives.
	OperatorNamespace string
}

// Reconcile is keyed on namespace/service-name (not workload name) so that a
// single reconciliation pass can collect all Ready workloads for a given
// Service and produce an authoritative EndpointSlice.
func (r *WorkloadRouteReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := ctrl.LoggerFrom(ctx).WithValues("service", req.NamespacedName)

	serviceName := req.Name
	namespace := req.Namespace

	// An empty service name is meaningless for this controller (a Workload
	// without spec.kubernetes.service.name should never have enqueued
	// here). Drop early as a defensive guard.
	if serviceName == "" {
		return ctrl.Result{}, nil
	}

	// Fetch the Service first. If it doesn't exist there is nothing to
	// reconcile — EndpointSlices we may have created carry an
	// OwnerReference on the Service, so Kubernetes GC handles cleanup
	// when the Service is deleted. Returning silently here avoids
	// spurious "service not found" logs for Workloads that don't define
	// a kubernetes.service block at all (their events can still reach
	// this reconciler via stale field-index entries during deletion).
	svc := &corev1.Service{}
	if err := r.Get(ctx, types.NamespacedName{Namespace: namespace, Name: serviceName}, svc); err != nil {
		if errors.IsNotFound(err) {
			return ctrl.Result{}, nil
		}
		return ctrl.Result{}, fmt.Errorf("getting service %s: %w", serviceName, err)
	}
	// GC removes the slice with the Service; a new one would be deleted and rebuilt in a loop.
	if !svc.DeletionTimestamp.IsZero() {
		return ctrl.Result{}, nil
	}

	// Collect all Workloads in this namespace that reference this service.
	workloadList := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, workloadList,
		client.InNamespace(namespace),
		client.MatchingFields{workloadKubernetesServiceIndex: serviceName},
	); err != nil {
		return ctrl.Result{}, fmt.Errorf("listing workloads for service %s: %w", serviceName, err)
	}

	// Deduplicate endpoints by pod IP. Multiple workloads on the same host
	// result in a single endpoint because the host's DynamicRouter handles
	// internal workload selection.
	type endpointInfo struct {
		podIP    string
		httpPort int32
	}
	endpoints := make(map[string]endpointInfo) // key = podIP

	for i := range workloadList.Items {
		workload := &workloadList.Items[i]
		if !workload.Status.IsAvailable() {
			continue
		}
		if workload.Status.HostID == "" {
			continue
		}

		// Look up the Host CRD by HostID.
		host, err := r.findHostByID(ctx, workload.Status.HostID)
		if err != nil {
			log.V(1).Info("host not found for workload, skipping", "hostID", workload.Status.HostID, "workload", workload.Name)
			continue
		}
		if host.Hostname == "" || host.HTTPPort == 0 {
			continue
		}

		// host.Hostname is the heartbeat's req.Hostname — documented as
		// the host pod's IP (see HostPodLabel comment in
		// host_pod_controller.go). EndpointSlice.AddressType=IPv4 (set
		// below) rejects non-IP entries with `Invalid value: "<x>":
		// must be a valid IPv4 address`, leaving the Service with zero
		// endpoints. When the host process reports its OS hostname
		// instead (default pod template, or any pod that pins
		// spec.hostname), translate hostname → pod IP via a Pod
		// lookup.
		podIP := host.Hostname
		if net.ParseIP(podIP) == nil {
			pod, err := r.findPodByHostname(ctx, host.Hostname)
			if err != nil {
				log.V(1).Info("pod lookup failed for non-IP host.Hostname, skipping workload", "hostID", workload.Status.HostID, "hostname", host.Hostname, "err", err)
				continue
			}
			if pod.Status.PodIP == "" {
				log.V(1).Info("matching pod has no PodIP yet, skipping workload", "hostID", workload.Status.HostID, "pod", pod.Name)
				continue
			}
			podIP = pod.Status.PodIP
		}
		httpPort := int32(host.HTTPPort)
		if _, exists := endpoints[podIP]; !exists {
			endpoints[podIP] = endpointInfo{podIP: podIP, httpPort: httpPort}
		}
	}

	sliceName := endpointSliceName(serviceName)

	if len(endpoints) == 0 {
		// No ready workloads for this service; delete our EndpointSlice if it exists.
		es := &discoveryv1.EndpointSlice{
			ObjectMeta: metav1.ObjectMeta{
				Name:      sliceName,
				Namespace: namespace,
			},
		}
		if err := r.Delete(ctx, es); err != nil && !errors.IsNotFound(err) {
			return ctrl.Result{}, fmt.Errorf("deleting empty EndpointSlice: %w", err)
		}
		log.Info("deleted EndpointSlice (no ready endpoints)", "endpointSlice", sliceName)
		return ctrl.Result{}, nil
	}

	// Set the port to the first HTTPPort, since all Hosts within the same HostGroup have the same HTTPPort.
	// Doing a range, since endpoints is a map object and can't use index.
	var port int32
	for _, ep := range endpoints {
		port = ep.httpPort
		break
	}

	portName := "http"
	proto := corev1.ProtocolTCP
	ready := true
	serving := true

	desiredSlice := &discoveryv1.EndpointSlice{
		ObjectMeta: metav1.ObjectMeta{
			Name:      sliceName,
			Namespace: namespace,
			Labels: map[string]string{
				discoveryv1.LabelServiceName: serviceName,
				routeManagerLabel:            routeManagedValue,
			},
			OwnerReferences: []metav1.OwnerReference{
				*metav1.NewControllerRef(svc, corev1.SchemeGroupVersion.WithKind("Service")),
			},
		},
		AddressType: discoveryv1.AddressTypeIPv4,
		Ports: []discoveryv1.EndpointPort{
			{
				Name:     &portName,
				Port:     &port,
				Protocol: &proto,
			},
		},
	}

	for _, ep := range endpoints {
		ip := ep.podIP
		desiredSlice.Endpoints = append(desiredSlice.Endpoints, discoveryv1.Endpoint{
			Addresses: []string{ip},
			Conditions: discoveryv1.EndpointConditions{
				Ready:   &ready,
				Serving: &serving,
			},
		})
	}

	// Normalize endpoint order for deterministic Create and stable comparison.
	sortEndpoints := func(eps []discoveryv1.Endpoint) {
		sort.Slice(eps, func(i, j int) bool {
			if len(eps[i].Addresses) == 0 || len(eps[j].Addresses) == 0 {
				return false
			}
			return eps[i].Addresses[0] < eps[j].Addresses[0]
		})
	}
	sortEndpoints(desiredSlice.Endpoints)

	existing := &discoveryv1.EndpointSlice{}
	err := r.Get(ctx, types.NamespacedName{Namespace: namespace, Name: sliceName}, existing)
	if errors.IsNotFound(err) {
		if err := r.Create(ctx, desiredSlice); err != nil {
			return ctrl.Result{}, fmt.Errorf("creating EndpointSlice: %w", err)
		}
		log.Info("created EndpointSlice", "endpointSlice", sliceName, "endpoints", len(endpoints))
		return ctrl.Result{}, nil
	}
	if err != nil {
		return ctrl.Result{}, err
	}

	sortEndpoints(existing.Endpoints)

	updated, err := r.updateEndpointSlice(ctx, existing, desiredSlice, svc)
	if err != nil {
		return ctrl.Result{}, err
	}
	if updated {
		log.Info("updated EndpointSlice", "endpointSlice", sliceName, "endpoints", len(endpoints))
	}

	return ctrl.Result{}, nil
}

// updateEndpointSlice writes desired into existing when they differ and reports
// whether it wrote. A deleting slice is left alone; only managed slices are adopted.
func (r *WorkloadRouteReconciler) updateEndpointSlice(ctx context.Context, existing, desired *discoveryv1.EndpointSlice, svc *corev1.Service) (bool, error) {
	// Adopting a deleting slice would not stop its deletion; its delete event requeues this Service.
	if !existing.DeletionTimestamp.IsZero() {
		ctrl.LoggerFrom(ctx).V(1).Info("waiting for EndpointSlice deletion to finish", "endpointSlice", existing.Name)
		return false, nil
	}
	if endpointSliceCurrent(existing, desired, svc) {
		return false, nil
	}
	if !metav1.IsControlledBy(existing, svc) {
		if existing.Labels[routeManagerLabel] != routeManagedValue {
			return false, fmt.Errorf("EndpointSlice %s exists and is not managed by this controller", existing.Name)
		}
		existing.OwnerReferences = desired.OwnerReferences
	}
	existing.Labels = desired.Labels
	existing.AddressType = desired.AddressType
	existing.Ports = desired.Ports
	existing.Endpoints = desired.Endpoints
	if err := r.Update(ctx, existing); err != nil {
		return false, fmt.Errorf("updating EndpointSlice: %w", err)
	}
	return true, nil
}

// endpointSliceCurrent reports whether existing matches desired and is owned by
// svc, not by a deleted Service of the same name that GC would follow.
func endpointSliceCurrent(existing, desired *discoveryv1.EndpointSlice, svc *corev1.Service) bool {
	return metav1.IsControlledBy(existing, svc) &&
		reflect.DeepEqual(existing.Labels, desired.Labels) &&
		existing.AddressType == desired.AddressType &&
		reflect.DeepEqual(existing.Ports, desired.Ports) &&
		reflect.DeepEqual(existing.Endpoints, desired.Endpoints)
}

// findPodByHostname locates the HostPodLabel-bearing Pod whose OS hostname
// matches the given hostname, matching first on Spec.Hostname (pinned pod
// templates), then on metadata.Name (default — pod name is also the
// container's OS hostname). The cluster-wide List is cache-bounded to
// the operator + host namespaces via ByObject in cmd/main.go.
func (r *WorkloadRouteReconciler) findPodByHostname(ctx context.Context, hostname string) (*corev1.Pod, error) {
	podList := &corev1.PodList{}
	if err := r.List(ctx, podList); err != nil {
		return nil, fmt.Errorf("listing pods: %w", err)
	}
	for i := range podList.Items {
		pod := &podList.Items[i]
		if _, ok := pod.Labels[HostPodLabel]; !ok {
			continue
		}
		if pod.Spec.Hostname == hostname || pod.Name == hostname {
			return pod, nil
		}
	}
	return nil, fmt.Errorf("no pod labelled %q matches hostname %q", HostPodLabel, hostname)
}

// findHostByID looks up a Host CRD by its HostID field using a field index.
// Hosts always live in the operator's namespace, so the lookup is
// namespace-scoped.
func (r *WorkloadRouteReconciler) findHostByID(ctx context.Context, hostID string) (*runtimev1alpha1.Host, error) {
	hostList := &runtimev1alpha1.HostList{}
	if err := r.List(ctx, hostList,
		client.InNamespace(r.OperatorNamespace),
		client.MatchingFields{hostIDIndex: hostID},
	); err != nil {
		return nil, err
	}
	if len(hostList.Items) == 0 {
		return nil, fmt.Errorf("no host found with hostID %s", hostID)
	}
	return &hostList.Items[0], nil
}

// endpointSliceName returns a deterministic, Kubernetes-safe name for the
// EndpointSlice associated with a given Service name. The result is at most
// 63 characters (the Kubernetes name limit).
func endpointSliceName(serviceName string) string {
	h := fnv.New32a()
	_, _ = h.Write([]byte(serviceName))
	suffix := fmt.Sprintf("%08x", h.Sum32())
	prefix := serviceName
	// 8 hex chars + 1 dash = 9 chars of suffix; keep prefix ≤ 54 chars.
	if len(prefix) > 54 {
		prefix = prefix[:54]
	}
	return fmt.Sprintf("%s-%s", prefix, suffix)
}

// SetupWithManager sets up the controller with the Manager.
// RBAC markers for this controller are declared at package level at the top
// of the file — see https://github.com/kubernetes-sigs/controller-tools for
// why declaration-attached markers can be silently skipped on some files.
func (r *WorkloadRouteReconciler) SetupWithManager(mgr ctrl.Manager) error {
	// Index Workloads by their KubernetesService name for efficient lookup.
	// Workloads without a kubernetes.service block, or with an empty
	// Service.Name, are excluded — they have no EndpointSlice to manage.
	if err := mgr.GetFieldIndexer().IndexField(
		context.Background(),
		&runtimev1alpha1.Workload{},
		workloadKubernetesServiceIndex,
		func(rawObj client.Object) []string {
			workload, ok := rawObj.(*runtimev1alpha1.Workload)
			if !ok || workload.Spec.Kubernetes == nil || workload.Spec.Kubernetes.Service == nil {
				return nil
			}
			if workload.Spec.Kubernetes.Service.Name == "" {
				return nil
			}
			return []string{workload.Spec.Kubernetes.Service.Name}
		},
	); err != nil {
		return err
	}

	// The Host-by-HostID index this controller reads is registered by
	// WorkloadReconciler, which needs it on every pass.

	// workloadToServiceRequest maps a Workload event to a reconcile request
	// keyed on the Workload's KubernetesService (namespace/service-name).
	// Workloads without a kubernetes.service block, or with an empty
	// Service.Name, do not enqueue — there is nothing for this reconciler
	// to do for them.
	workloadToServiceRequest := func(_ context.Context, obj client.Object) []reconcile.Request {
		workload, ok := obj.(*runtimev1alpha1.Workload)
		if !ok || workload.Spec.Kubernetes == nil || workload.Spec.Kubernetes.Service == nil {
			return nil
		}
		if workload.Spec.Kubernetes.Service.Name == "" {
			return nil
		}
		return []reconcile.Request{
			{NamespacedName: types.NamespacedName{
				Namespace: workload.Namespace,
				Name:      workload.Spec.Kubernetes.Service.Name,
			}},
		}
	}

	return ctrl.NewControllerManagedBy(mgr).
		Watches(&runtimev1alpha1.Workload{}, handler.EnqueueRequestsFromMapFunc(workloadToServiceRequest)).
		Watches(&corev1.Service{}, handler.EnqueueRequestsFromMapFunc(r.serviceToRequest)).
		Watches(&discoveryv1.EndpointSlice{}, handler.EnqueueRequestsFromMapFunc(endpointSliceToRequest),
			builder.WithPredicates(managedSliceDeleted)).
		Named("workload-route").
		Complete(r)
}

// serviceToRequest enqueues a Service only when a Workload routes to it, so
// the other Services in the cluster are never reconciled.
func (r *WorkloadRouteReconciler) serviceToRequest(ctx context.Context, obj client.Object) []reconcile.Request {
	request := []reconcile.Request{{NamespacedName: types.NamespacedName{
		Namespace: obj.GetNamespace(),
		Name:      obj.GetName(),
	}}}
	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, workloads,
		client.InNamespace(obj.GetNamespace()),
		client.MatchingFields{workloadKubernetesServiceIndex: obj.GetName()},
	); err != nil {
		// Reconciling is idempotent; an extra pass is cheaper than a lost one.
		return request
	}
	if len(workloads.Items) == 0 {
		return nil
	}
	return request
}

// managedSliceDeleted passes only the deletion of a slice this controller
// manages, so its Service is reconciled again when the slice goes away.
var managedSliceDeleted = predicate.Funcs{
	CreateFunc: func(event.CreateEvent) bool { return false },
	UpdateFunc: func(event.UpdateEvent) bool { return false },
	DeleteFunc: func(e event.DeleteEvent) bool {
		return e.Object.GetLabels()[routeManagerLabel] == routeManagedValue
	},
	GenericFunc: func(event.GenericEvent) bool { return false },
}

// endpointSliceToRequest maps a managed slice to its Service.
func endpointSliceToRequest(_ context.Context, obj client.Object) []reconcile.Request {
	name := obj.GetLabels()[discoveryv1.LabelServiceName]
	if name == "" {
		return nil
	}
	return []reconcile.Request{{NamespacedName: types.NamespacedName{Namespace: obj.GetNamespace(), Name: name}}}
}
