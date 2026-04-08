package runtime

import (
	"context"
	"fmt"
	"hash/fnv"
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
}

// Reconcile is keyed on namespace/service-name (not workload name) so that a
// single reconciliation pass can collect all Ready workloads for a given
// Service and produce an authoritative EndpointSlice.
func (r *WorkloadRouteReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := ctrl.LoggerFrom(ctx).WithValues("service", req.NamespacedName)

	serviceName := req.Name
	namespace := req.Namespace

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

		// Hostname is actually the pod IP of the host's Pod, so we can use it directly as the
		// Endpoint address without needing to fetch the Pod object
		podIP := host.Hostname
		httpPort := int32(host.HTTPPort)
		if _, exists := endpoints[podIP]; !exists {
			endpoints[podIP] = endpointInfo{podIP: podIP, httpPort: httpPort}
		}
	}

	// Fetch the Service so we can set it as the owner of the EndpointSlice.
	svc := &corev1.Service{}
	if err := r.Get(ctx, types.NamespacedName{Namespace: namespace, Name: serviceName}, svc); err != nil {
		if errors.IsNotFound(err) {
			log.V(1).Info("service not found, skipping EndpointSlice reconciliation")
			return ctrl.Result{}, nil
		}
		return ctrl.Result{}, fmt.Errorf("getting service %s: %w", serviceName, err)
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

	if reflect.DeepEqual(existing.Labels, desiredSlice.Labels) &&
		existing.AddressType == desiredSlice.AddressType &&
		reflect.DeepEqual(existing.Ports, desiredSlice.Ports) &&
		reflect.DeepEqual(existing.Endpoints, desiredSlice.Endpoints) {
		return ctrl.Result{}, nil
	}

	// Update if the existing slice differs.
	existing.Labels = desiredSlice.Labels
	existing.AddressType = desiredSlice.AddressType
	existing.Ports = desiredSlice.Ports
	existing.Endpoints = desiredSlice.Endpoints
	if err := r.Update(ctx, existing); err != nil {
		return ctrl.Result{}, fmt.Errorf("updating EndpointSlice: %w", err)
	}
	log.Info("updated EndpointSlice", "endpointSlice", sliceName, "endpoints", len(endpoints))

	return ctrl.Result{}, nil
}

// findHostByID looks up a Host CRD by its HostID field using a field index.
func (r *WorkloadRouteReconciler) findHostByID(ctx context.Context, hostID string) (*runtimev1alpha1.Host, error) {
	hostList := &runtimev1alpha1.HostList{}
	if err := r.List(ctx, hostList, client.MatchingFields{hostIDIndex: hostID}); err != nil {
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
// +kubebuilder:rbac:groups=discovery.k8s.io,resources=endpointslices,verbs=create;delete;get;list;patch;update;watch
// +kubebuilder:rbac:groups="",resources=services,verbs=get;list;watch
func (r *WorkloadRouteReconciler) SetupWithManager(mgr ctrl.Manager) error {
	// Index Workloads by their KubernetesService name for efficient lookup.
	if err := mgr.GetFieldIndexer().IndexField(
		context.Background(),
		&runtimev1alpha1.Workload{},
		workloadKubernetesServiceIndex,
		func(rawObj client.Object) []string {
			workload, ok := rawObj.(*runtimev1alpha1.Workload)
			if !ok || workload.Spec.Kubernetes == nil || workload.Spec.Kubernetes.Service == nil {
				return nil
			}
			return []string{workload.Spec.Kubernetes.Service.Name}
		},
	); err != nil {
		return err
	}

	// Index Hosts by HostID for O(1) lookup from workload.Status.HostID.
	if err := mgr.GetFieldIndexer().IndexField(
		context.Background(),
		&runtimev1alpha1.Host{},
		hostIDIndex,
		func(rawObj client.Object) []string {
			host, ok := rawObj.(*runtimev1alpha1.Host)
			if !ok || host.HostID == "" {
				return nil
			}
			return []string{host.HostID}
		},
	); err != nil {
		return err
	}

	// workloadToServiceRequest maps a Workload event to a reconcile request
	// keyed on the Workload's KubernetesService (namespace/service-name).
	workloadToServiceRequest := func(_ context.Context, obj client.Object) []reconcile.Request {
		workload, ok := obj.(*runtimev1alpha1.Workload)
		if !ok || workload.Spec.Kubernetes == nil || workload.Spec.Kubernetes.Service == nil {
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
		Named("workload-route").
		Complete(r)
}
