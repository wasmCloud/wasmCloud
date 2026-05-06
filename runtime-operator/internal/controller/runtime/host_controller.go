package runtime

import (
	"context"
	"fmt"
	"strconv"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

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
		host := &runtimev1alpha1.Host{
			ObjectMeta: metav1.ObjectMeta{
				Name:      req.FriendlyName,
				Namespace: h.operatorNamespace,
			},
		}

		// CreateOrUpdate handles spec and metadata (labels, HostID, Hostname,
		// HTTPPort, Environment). The mutate func sets these fields so they
		// are applied on both create and update paths (c.Get overwrites the
		// pre-populated struct for existing objects).
		_, err := controllerutil.CreateOrUpdate(ctx, h.client, host, func() error {
			host.Labels = req.GetLabels()
			host.HostID = req.Id
			host.Hostname = req.Hostname
			host.HTTPPort = req.HttpPort
			host.Environment = req.GetEnvironment()
			return nil
		})
		if err != nil {
			log.Error(err, "failed to create or update Host resource", "host", req.FriendlyName, "hostID", req.Id)
			return
		}

		// Status is a separate subresource; c.Update() (used by CreateOrUpdate)
		// silently ignores status fields. Use Status().Patch() to persist
		// LastSeen without conflicting with concurrent metadata changes (e.g.
		// the ConditionedReconciler adding a finalizer between CreateOrUpdate
		// and this call).
		base := host.DeepCopy()
		host.Status.LastSeen = metav1.Now()
		if err := h.client.Status().Patch(ctx, host, client.MergeFrom(base)); err != nil {
			log.Error(err, "failed to update Host status", "host", req.FriendlyName, "hostID", req.Id)
		}
	})

	<-ctx.Done()
	return subscription.Drain()
}
