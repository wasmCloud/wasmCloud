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
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/handler"
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
	seenHosts := make(map[string]bool)

	for i := range workloadList.Items {
		workload := &workloadList.Items[i]
		if !workload.DeletionTimestamp.IsZero() || !workload.Status.IsAvailable() {
			continue
		}
		if workload.Status.HostID == "" {
			continue
		}

		if seenHosts[workload.Status.HostID] {
			continue
		}
		seenHosts[workload.Status.HostID] = true
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
				routeManagerLabel:            "true",
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

	return ctrl.Result{}, r.syncSlices(ctx, desiredSlice)
}

// Preserve existing membership so deleting one endpoint changes only its slice.
// Fill available capacity before allocating another bounded slice.
func (r *WorkloadRouteReconciler) syncSlices(ctx context.Context, desired *discoveryv1.EndpointSlice) error {
	list := &discoveryv1.EndpointSliceList{}
	if err := r.List(ctx, list, client.InNamespace(desired.Namespace), client.MatchingLabels(desired.Labels)); err != nil {
		return err
	}
	// Read the legacy deterministic name too, so damaged labels can be repaired.
	legacy := &discoveryv1.EndpointSlice{}
	if err := r.Get(ctx, client.ObjectKeyFromObject(desired), legacy); err == nil {
		found := false
		for i := range list.Items {
			if list.Items[i].Name == legacy.Name {
				found = true
				break
			}
		}
		if !found {
			list.Items = append(list.Items, *legacy)
		}
	} else if !errors.IsNotFound(err) {
		return err
	}
	sort.Slice(list.Items, func(i, j int) bool { return list.Items[i].Name < list.Items[j].Name })
	remaining := make(map[string]discoveryv1.Endpoint, len(desired.Endpoints))
	for _, ep := range desired.Endpoints {
		remaining[ep.Addresses[0]] = ep
	}
	slices := make([]*discoveryv1.EndpointSlice, 0, len(list.Items))
	occupied := make(map[string]bool)
	for i := range list.Items {
		current := &list.Items[i]
		occupied[current.Name] = true
		if len(current.OwnerReferences) > 0 && !reflect.DeepEqual(current.OwnerReferences, desired.OwnerReferences) {
			if err := deleteObservedObject(ctx, r.Client, current); err != nil {
				return err
			}
			continue
		}
		next := desired.DeepCopy()
		next.Name = current.Name
		next.ResourceVersion = current.ResourceVersion
		next.Endpoints = nil
		for _, ep := range current.Endpoints {
			if len(ep.Addresses) != 1 || len(next.Endpoints) >= 100 {
				continue
			}
			if wanted, ok := remaining[ep.Addresses[0]]; ok {
				next.Endpoints = append(next.Endpoints, wanted)
				delete(remaining, ep.Addresses[0])
			}
		}
		slices = append(slices, next)
	}
	addresses := make([]string, 0, len(remaining))
	for address := range remaining {
		addresses = append(addresses, address)
	}
	sort.Strings(addresses)
	slot := 0
	for _, address := range addresses {
		for slot < len(slices) && len(slices[slot].Endpoints) >= 100 {
			slot++
		}
		if slot == len(slices) {
			next := desired.DeepCopy()
			next.Endpoints = nil
			for suffix := 0; ; suffix++ {
				next.Name = desired.Name
				if suffix > 0 {
					next.Name = endpointSliceName(fmt.Sprintf("%s-%d", desired.Name, suffix))
				}
				if !occupied[next.Name] {
					break
				}
			}
			occupied[next.Name] = true
			slices = append(slices, next)
		}
		slices[slot].Endpoints = append(slices[slot].Endpoints, remaining[address])
	}
	return r.applySlices(ctx, list, slices)
}

func (r *WorkloadRouteReconciler) applySlices(
	ctx context.Context, list *discoveryv1.EndpointSliceList, slices []*discoveryv1.EndpointSlice,
) error {
	existing := make(map[string]*discoveryv1.EndpointSlice, len(list.Items))
	for i := range list.Items {
		existing[list.Items[i].Name] = &list.Items[i]
	}
	for _, next := range slices {
		current := existing[next.Name]
		if len(next.Endpoints) == 0 {
			if current != nil {
				if err := deleteObservedObject(ctx, r.Client, current); err != nil {
					return err
				}
			}
			continue
		}
		sort.Slice(next.Endpoints, func(i, j int) bool { return next.Endpoints[i].Addresses[0] < next.Endpoints[j].Addresses[0] })
		if current == nil {
			if err := r.Create(ctx, next); err != nil {
				return err
			}
			continue
		}
		sort.Slice(current.Endpoints, func(i, j int) bool {
			if len(current.Endpoints[i].Addresses) == 0 || len(current.Endpoints[j].Addresses) == 0 {
				return len(current.Endpoints[i].Addresses) < len(current.Endpoints[j].Addresses)
			}
			return current.Endpoints[i].Addresses[0] < current.Endpoints[j].Addresses[0]
		})
		if reflect.DeepEqual(current.Labels, next.Labels) && reflect.DeepEqual(current.OwnerReferences, next.OwnerReferences) && reflect.DeepEqual(current.Endpoints, next.Endpoints) && reflect.DeepEqual(current.Ports, next.Ports) && current.AddressType == next.AddressType {
			continue
		}
		current.Labels, current.OwnerReferences = next.Labels, next.OwnerReferences
		current.Endpoints, current.Ports, current.AddressType = next.Endpoints, next.Ports, next.AddressType
		if err := r.Update(ctx, current); err != nil {
			return err
		}
	}
	return nil
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
		Watches(&runtimev1alpha1.Workload{}, coalesce(handler.EnqueueRequestsFromMapFunc(workloadToServiceRequest))).
		Watches(&corev1.Service{}, coalesce(&handler.EnqueueRequestForObject{})).
		Named("workload-route").
		Complete(r)
}
