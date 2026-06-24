package runtime

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
)

const (
	hostHeartbeatTimeout  = 5 * time.Second
	hostReconcileInterval = 1 * time.Minute
	hostFinalizerName     = "runtime.wasmcloud.dev/host-finalizer"
	// workloadByHostIDIndex indexes Workloads by their Status.HostID so
	// the host finalizer can fan out to assigned workloads without
	// scanning every Workload in the cluster.
	workloadByHostIDIndex = "status.hostId"
)

// HostReconciler reconciles a Host object
type HostReconciler struct {
	client.Client
	Scheme             *runtime.Scheme
	Bus                wasmbus.Bus
	UnreachableTimeout time.Duration
	CPUThreshold       float64
	MemoryThreshold    float64
	// OperatorNamespace is the namespace the operator itself runs in. Every
	// Host object is created here regardless of where the underlying host
	// pod runs; tenant attribution lives on the Host's Environment field.
	OperatorNamespace string

	reconciler condition.AnyConditionedReconciler
}

func (r *HostReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	return r.reconciler.Reconcile(ctx, req)
}

func (r *HostReconciler) reconcileReporting(ctx context.Context, host *runtimev1alpha1.Host) error {
	client := NewWashHostClient(r.Bus, host.HostID)

	ctx, cancel := context.WithTimeout(ctx, hostHeartbeatTimeout)
	defer cancel()

	heartbeat, err := client.Heartbeat(ctx)
	if err != nil {
		return err
	}

	condition.ForceStatusUpdate(ctx)

	host.Status.SystemCPUUsage = strconv.FormatFloat(float64(heartbeat.GetSystemCpuUsage()), 'f', -1, 32)
	host.Status.SystemMemoryTotal = int64(heartbeat.GetSystemMemoryTotal())
	host.Status.SystemMemoryFree = int64(heartbeat.GetSystemMemoryFree())
	host.Status.ComponentCount = int(heartbeat.GetComponentCount())
	host.Status.WorkloadCount = int(heartbeat.GetWorkloadCount())
	host.Status.OSName = heartbeat.GetOsName()
	host.Status.OSArch = heartbeat.GetOsArch()
	host.Status.OSKernel = heartbeat.GetOsKernel()
	host.Status.Version = heartbeat.GetVersion()

	host.Status.LastSeen = metav1.Now()
	return nil
}

func (r *HostReconciler) reconcileReady(_ context.Context, host *runtimev1alpha1.Host) error {
	if host.Status.AllTrue(runtimev1alpha1.HostConditionReporting) {
		return nil
	}

	if host.Status.LastSeen.Add(r.UnreachableTimeout).After(metav1.Now().Time) {
		return condition.ErrStatusUnknown(fmt.Errorf("host is not reporting"))
	}

	return fmt.Errorf("host has not reported recently")
}

// finalize deletes all Workload objects assigned to this host so that the
// ReplicaSet controller can immediately schedule replacements rather than
// waiting for the unhealthy-workload grace period to expire.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloads,verbs=get;list;delete
func (r *HostReconciler) finalize(ctx context.Context, host *runtimev1alpha1.Host) error {
	// Indexed list keyed on Status.HostID avoids scanning every Workload
	// in the cluster when a host is finalized. The list is cluster-wide
	// because Workloads live in tenant namespaces while Hosts live in the
	// operator's namespace.
	workloadList := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, workloadList,
		client.MatchingFields{workloadByHostIDIndex: host.HostID},
	); err != nil {
		return err
	}

	for i := range workloadList.Items {
		workload := &workloadList.Items[i]
		if workload.DeletionTimestamp != nil {
			continue
		}
		if err := r.Delete(ctx, workload); client.IgnoreNotFound(err) != nil {
			return err
		}
	}

	return nil
}

// SetupWithManager sets up the controller with the Manager.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts/finalizers,verbs=update

func (r *HostReconciler) SetupWithManager(mgr ctrl.Manager) error {
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.Host{},
		hostReconcileInterval)

	reconciler.SetFinalizer(hostFinalizerName, r.finalize)
	reconciler.SetCondition(runtimev1alpha1.HostConditionReporting, r.reconcileReporting)
	reconciler.SetCondition(condition.TypeReady, r.reconcileReady)

	reconciler.AddPostHook(func(ctx context.Context, host *runtimev1alpha1.Host) error {
		if host.Status.AllTrue(condition.TypeReady) {
			return nil
		}
		if host.Status.AnyUnknown(condition.TypeReady) {
			return nil
		}
		// Delete unresponsive host; the finalizer will clean up assigned workloads.
		return r.Delete(ctx, host)
	})

	r.reconciler = reconciler

	statusUpdater := &hostStatusUpdater{
		bus:               r.Bus,
		client:            r.Client,
		operatorNamespace: r.OperatorNamespace,
	}
	if err := mgr.Add(statusUpdater); err != nil {
		return err
	}

	// Index Workloads by Status.HostID so finalize can fan out to all
	// workloads assigned to a host via a direct field-indexed list rather
	// than scanning every Workload in the cluster.
	if err := mgr.GetFieldIndexer().IndexField(
		context.Background(),
		&runtimev1alpha1.Workload{},
		workloadByHostIDIndex,
		func(obj client.Object) []string {
			workload, ok := obj.(*runtimev1alpha1.Workload)
			if !ok || workload.Status.HostID == "" {
				return nil
			}
			return []string{workload.Status.HostID}
		},
	); err != nil {
		return err
	}

	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.Host{}).
		Named("workload-host").
		Complete(r)
}

type hostStatusUpdater struct {
	bus    wasmbus.Bus
	client client.Client
	// operatorNamespace is the namespace every Host object is created in.
	operatorNamespace string
}

func (h *hostStatusUpdater) Start(ctx context.Context) error {
	subscription, err := h.bus.Subscribe("runtime.operator.heartbeat.>", 100)
	if err != nil {
		return err
	}

	log := ctrl.LoggerFrom(ctx).WithName("host-status-updater")

	subscription.Handle(func(msg *wasmbus.Message) {
		var req runtimev2.HostHeartbeat
		if err := protojson.Unmarshal(msg.Data, &req); err != nil {
			log.Error(err, "failed to decode heartbeat message")
			return
		}

		// Every Host object lives in the operator's own namespace. Tenant
		// attribution is recorded on the Host's Environment field,
		// populated verbatim from req.Environment — the heartbeat is the
		// source of truth and is not validated against cluster state.
		//
		// Upsert spec and metadata (labels, HostID, Hostname, HTTPPort,
		// Environment) with Server-Side Apply. SSA performs the create-or-update
		// at the API server with no client-side Get, so it is immune to
		// informer-cache staleness: a read-modify-write (e.g. CreateOrUpdate)
		// can see a stale NotFound, take the Create path, and fail with
		// AlreadyExists when the object actually exists — SSA cannot. TypeMeta
		// must be set for Apply, and a stable FieldOwner keeps this handler's
		// fields cleanly separated from those owned by the reconciler.
		host := &runtimev1alpha1.Host{
			TypeMeta: metav1.TypeMeta{
				APIVersion: runtimev1alpha1.GroupVersion.String(),
				Kind:       "Host",
			},
			ObjectMeta: metav1.ObjectMeta{
				Name:      req.FriendlyName,
				Namespace: h.operatorNamespace,
				Labels:    req.GetLabels(),
			},
			HostID:      req.Id,
			Hostname:    req.Hostname,
			HTTPPort:    req.HttpPort,
			Environment: req.GetEnvironment(),
		}
		if err := h.client.Patch(ctx, host, client.Apply,
			client.FieldOwner("host-status-updater"), client.ForceOwnership); err != nil {
			log.Error(err, "failed to apply Host resource", "host", req.FriendlyName, "hostID", req.Id)
			return
		}

		// Status is a separate subresource; c.Update() (used by CreateOrUpdate)
		// silently ignores status fields. Patch the status subresource to
		// persist LastSeen without conflicting with concurrent metadata
		// changes (e.g. the ConditionedReconciler adding a finalizer between
		// CreateOrUpdate and this call).
		//
		// The Host CRD marks the system/OS status fields as required, so the
		// status subresource is rejected unless those keys are present — yet
		// reconcileReporting (a separate RPC poll of the host) is what fills
		// in their real values, which can lag or never happen for an
		// unresponsive host. A diff-based MergeFrom patch additionally drops
		// fields whose value equals the base (an int64 0 is omitted), so the
		// keys never get written. Build the status patch explicitly so it
		// always carries the required keys, injecting zero-value defaults only
		// for fields not yet reported (a zero value satisfies "required"
		// without clobbering any real value). Conditions and the optional
		// counts are intentionally excluded so the ConditionedReconciler's
		// state is left untouched.
		statusPatch, err := hostStatusPatch(&host.Status)
		if err != nil {
			log.Error(err, "failed to build Host status patch", "host", req.FriendlyName, "hostID", req.Id)
			return
		}
		if err := h.client.Status().Patch(ctx, host, client.RawPatch(types.MergePatchType, statusPatch)); err != nil {
			log.Error(err, "failed to update Host status", "host", req.FriendlyName, "hostID", req.Id)
		}
	})

	<-ctx.Done()
	return subscription.Drain()
}

// hostStatusPatch builds a JSON merge patch for the Host status subresource
// that always refreshes LastSeen and guarantees the CRD-required system/OS
// fields are present. Those fields are populated with real values by
// reconcileReporting's RPC poll; until that succeeds (and it may never, for an
// unresponsive host) the keys would be absent and the API server rejects the
// status write with "Required value". A zero value satisfies the required
// constraint, so each required field is defaulted only when it has not yet been
// reported — never overwriting a real value already present on the object.
// Conditions and the optional counts are deliberately omitted so the patch does
// not clobber state owned by the ConditionedReconciler.
func hostStatusPatch(status *runtimev1alpha1.HostStatus) ([]byte, error) {
	s := map[string]any{"lastSeen": metav1.Now()}
	if status.Version == "" {
		s["version"] = "unknown"
	}
	if status.OSName == "" {
		s["osName"] = "unknown"
	}
	if status.OSArch == "" {
		s["osArch"] = "unknown"
	}
	if status.OSKernel == "" {
		s["osKernel"] = "unknown"
	}
	if status.SystemCPUUsage == "" {
		s["systemCPUUsage"] = "0"
	}
	if status.SystemMemoryTotal == 0 {
		s["systemMemoryTotal"] = 0
	}
	if status.SystemMemoryFree == 0 {
		s["systemMemoryFree"] = 0
	}
	return json.Marshal(map[string]any{"status": s})
}
