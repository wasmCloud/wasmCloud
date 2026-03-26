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
)

// HostReconciler reconciles a Host object
type HostReconciler struct {
	client.Client
	Scheme             *runtime.Scheme
	Bus                wasmbus.Bus
	UnreachableTimeout time.Duration
	CPUThreshold       float64
	MemoryThreshold    float64

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
	workloadList := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, workloadList); err != nil {
		return err
	}

	for i := range workloadList.Items {
		workload := &workloadList.Items[i]
		if workload.Status.HostID != host.HostID {
			continue
		}
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
		bus:    r.Bus,
		client: r.Client,
	}
	if err := mgr.Add(statusUpdater); err != nil {
		return err
	}

	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.Host{}).
		Named("workload-Host").
		Complete(r)
}

type hostStatusUpdater struct {
	bus    wasmbus.Bus
	client client.Client
}

func (h *hostStatusUpdater) Start(ctx context.Context) error {
	subscription, err := h.bus.Subscribe("runtime.operator.heartbeat.>", 100)
	if err != nil {
		return err
	}

	go subscription.Handle(func(msg *wasmbus.Message) {
		var req runtimev2.HostHeartbeat
		if err := protojson.Unmarshal(msg.Data, &req); err != nil {
			fmt.Println("Failed to decode heartbeat message:", err)
			return
		}

		host := &runtimev1alpha1.Host{
			ObjectMeta: metav1.ObjectMeta{
				Name: req.FriendlyName,
			},
		}

		// CreateOrUpdate handles spec and metadata (labels, HostID, Hostname,
		// HTTPPort). The mutate func sets these fields so they are applied on
		// both create and update paths (c.Get overwrites the pre-populated
		// struct for existing objects).
		_, err := controllerutil.CreateOrUpdate(ctx, h.client, host, func() error {
			host.Labels = req.GetLabels()
			host.HostID = req.Id
			host.Hostname = req.Hostname
			host.HTTPPort = req.HttpPort
			return nil
		})
		if err != nil {
			fmt.Println("Failed to create or update Host resource:", err)
			return
		}

		// Status is a separate subresource; c.Update() (used by CreateOrUpdate)
		// silently ignores status fields. Use Status().Update() to persist LastSeen.
		host.Status.LastSeen = metav1.Now()
		if err := h.client.Status().Update(ctx, host); err != nil {
			fmt.Println("Failed to update Host status:", err)
		}
	})

	<-ctx.Done()
	return subscription.Drain()
}
