package runtime

import (
	"context"
	"encoding/json"
	"fmt"
	"maps"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/go-logr/logr"
	"golang.org/x/sync/errgroup"
	"google.golang.org/protobuf/encoding/protojson"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/util/retry"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller"
	"sigs.k8s.io/controller-runtime/pkg/predicate"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
)

const (
	hostHeartbeatTimeout          = 5 * time.Second
	hostReconcileInterval         = 1 * time.Minute
	hostFinalizerName             = "runtime.wasmcloud.dev/host-finalizer"
	hostFinalizeDeleteConcurrency = 24
	// workloadByHostIDIndex indexes Workloads by their Status.HostID so
	// the host finalizer can fan out to assigned workloads without
	// scanning every Workload in the cluster.
	workloadByHostIDIndex = "status.hostId"
)

// HostReconciler reconciles a Host object
type HostReconciler struct {
	client.Client
	apiReader          client.Reader
	Scheme             *runtime.Scheme
	Bus                wasmbus.Bus
	UnreachableTimeout time.Duration
	CPUThreshold       float64
	MemoryThreshold    float64
	// OperatorNamespace is the namespace the operator itself runs in. Every
	// Host object is created here regardless of where the underlying host
	// pod runs; tenant attribution lives on the Host's Environment field.
	OperatorNamespace string
	// RetiredHosts rejects heartbeats from removed Hosts and Pods.
	RetiredHosts *RetiredHosts

	// fleet gates host deletion; the heartbeat subscription writes to it.
	fleet fleetWitness

	reconciler condition.AnyConditionedReconciler
}

// fleetWitness records whether the operator is hearing the fleet.
//
// A host's probe and its pushed heartbeats both travel over the operator's one
// NATS connection, so a deaf operator sees every host fall silent at once and
// deleteUnresponsiveHost deletes them all, each finalizer taking its Workloads
// with it. Arrival is the evidence, not the connection's own state: NATS can
// hold the socket up while it has lost its routes to the hosts, or has dropped
// this subscription for overrunning its backlog.
//
// The signal is fleet-wide, so it catches total deafness only. A partition that
// isolates some hosts while others keep publishing still reaps the unreachable
// ones, as it did before this existed.
type fleetWitness struct {
	mu sync.Mutex
	// lastHeard is when a heartbeat from any host last arrived.
	lastHeard time.Time
	// heardSince is when the operator resumed hearing the fleet after its most
	// recent gap, or the zero time while it is hearing nothing.
	heardSince time.Time
}

// heard records a heartbeat arriving from some host.
func (w *fleetWitness) heard(now time.Time) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.lastHeard = now
}

// observe advances the grace period, or clears it if nothing has been heard
// within window. It must run on every reconcile rather than only when a host
// looks silent, or the clock reads zero through a healthy uptime and the first
// host to genuinely die waits an extra window to be reaped.
func (w *fleetWitness) observe(now time.Time, window time.Duration) {
	w.mu.Lock()
	defer w.mu.Unlock()

	if w.lastHeard.IsZero() || now.Sub(w.lastHeard) >= window {
		w.heardSince = time.Time{}
		return
	}
	if w.heardSince.IsZero() {
		w.heardSince = now
	}
}

// settled reports whether the operator has been hearing the fleet without a gap
// for a full window. It re-checks arrival itself rather than trusting the last
// observe: callers reach it after a heartbeat probe that can burn
// hostHeartbeatTimeout, and the fleet can fall silent inside that.
//
// The gap is what makes recent arrival alone insufficient: hosts publish every
// 15s, so for that long after the bus returns a live host has yet to be heard
// from while its neighbours have already ticked. Requiring the whole window
// keeps the ones that have not ticked from being condemned on their neighbours.
func (w *fleetWitness) settled(now time.Time, window time.Duration) bool {
	w.mu.Lock()
	defer w.mu.Unlock()

	if w.lastHeard.IsZero() || now.Sub(w.lastHeard) >= window {
		return false
	}
	if w.heardSince.IsZero() {
		return false
	}
	return now.Sub(w.heardSince) >= window
}

func (r *HostReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	return r.reconciler.Reconcile(ctx, req)
}

func (r *HostReconciler) reconcileReporting(ctx context.Context, host *runtimev1alpha1.Host) error {
	// Reporting is the first condition, so this runs on every reconcile pass
	// that can reach the readiness verdict.
	r.fleet.observe(time.Now(), r.UnreachableTimeout)

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

	now := time.Now()
	if host.Status.LastSeen.Add(r.UnreachableTimeout).After(now) {
		return condition.ErrStatusUnknown(fmt.Errorf("host is not reporting"))
	}

	// A definite failure here costs the host every Workload on it, so it is
	// only reachable once the operator has been hearing the rest of the fleet
	// long enough that this host's silence is the host's own.
	if !r.fleet.settled(now, r.UnreachableTimeout) {
		return condition.ErrStatusUnknown(
			fmt.Errorf("host has not reported recently, but the operator has not been hearing the fleet for %s", r.UnreachableTimeout))
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
	//
	// UnsafeDisableDeepCopy makes the items share the informer cache's memory
	// instead of being deep copied,
	// which a host with many Workloads would pay for on every pass.
	// That is safe here because the items are only read:
	//   - finalize and deleteAssignedWorkload read their identity, assignment
	//     and deletion timestamp, and never assign to them.
	//   - Delete builds its request from the object's name and namespace and
	//     does not decode the response back into it.
	//   - The informer replaces a cached object on update rather than changing
	//     it in place, so these reads do not race with the cache.
	// Anything added below that writes to a listed Workload must DeepCopy it first.
	workloadList := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, workloadList,
		client.MatchingFields{workloadByHostIDIndex: host.HostID},
		client.UnsafeDisableDeepCopy,
	); err != nil {
		return err
	}

	deletes, deleteCtx := errgroup.WithContext(ctx)
	deletes.SetLimit(hostFinalizeDeleteConcurrency)
	for i := range workloadList.Items {
		workload := &workloadList.Items[i]
		if workload.DeletionTimestamp != nil {
			continue
		}
		if deleteCtx.Err() != nil {
			break
		}
		deletes.Go(func() error {
			if err := deleteCtx.Err(); err != nil {
				return err
			}
			return r.deleteAssignedWorkload(deleteCtx, workload, host.HostID)
		})
	}

	if err := deletes.Wait(); err != nil {
		return err
	}
	return ctx.Err()
}

// Check both object identity and the observed assignment when deleting. A
// conflict is retried against an uncached read, since assignment or unrelated
// status may have changed since the index was read.
func (r *HostReconciler) deleteAssignedWorkload(ctx context.Context, workload *runtimev1alpha1.Workload, hostID string) error {
	snapshot := workload
	key := client.ObjectKeyFromObject(workload)
	return retry.RetryOnConflict(retry.DefaultRetry, func() error {
		if snapshot == nil {
			snapshot = &runtimev1alpha1.Workload{}
			if err := r.apiReader.Get(ctx, key, snapshot); err != nil {
				return client.IgnoreNotFound(err)
			}
		}
		if snapshot.UID != workload.UID || snapshot.Status.HostID != hostID || snapshot.DeletionTimestamp != nil {
			return nil
		}
		uid, resourceVersion := snapshot.UID, snapshot.ResourceVersion
		err := r.Delete(ctx, snapshot, client.Preconditions{UID: &uid, ResourceVersion: &resourceVersion})
		if apierrors.IsConflict(err) {
			snapshot = nil
		}
		return client.IgnoreNotFound(err)
	})
}

// deleteUnresponsiveHost is the reconcile post-hook: a Host whose Ready
// condition has resolved to a definite not-ready (neither True nor Unknown)
// is deleted, and its finalizer cleans up assigned workloads. While Ready is
// still Unknown the host is left in place so a recovering host is not deleted
// out from under in-flight heartbeats.
func (r *HostReconciler) deleteUnresponsiveHost(ctx context.Context, host *runtimev1alpha1.Host) error {
	if host.Status.AllTrue(condition.TypeReady) {
		return nil
	}
	if host.Status.AnyUnknown(condition.TypeReady) {
		return nil
	}
	// Delete unresponsive host; the finalizer will clean up assigned workloads.
	return r.Delete(ctx, host)
}

// SetupWithManager sets up the controller with the Manager.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=hosts/finalizers,verbs=update

func (r *HostReconciler) SetupWithManager(mgr ctrl.Manager) error {
	r.apiReader = mgr.GetAPIReader()
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.Host{},
		hostReconcileInterval)

	reconciler.SetFinalizer(hostFinalizerName, r.finalize)
	reconciler.SetCondition(runtimev1alpha1.HostConditionReporting, r.reconcileReporting)
	reconciler.SetCondition(condition.TypeReady, r.reconcileReady)

	reconciler.AddPostHook(r.deleteUnresponsiveHost)

	r.reconciler = reconciler

	statusUpdater := &hostStatusUpdater{
		bus:               r.Bus,
		client:            r.Client,
		operatorNamespace: r.OperatorNamespace,
		fleet:             &r.fleet,
		retiredHosts:      r.RetiredHosts,
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

	// Keep a worker available to register a replacement while the previous
	// Host is deleting its assigned Workloads. Respect explicit manager tuning.
	managerOptions := mgr.GetControllerOptions()
	hostOptions := controller.Options{}
	hostGroupKind := runtimev1alpha1.GroupVersion.WithKind("Host").GroupKind().String()
	if managerOptions.MaxConcurrentReconciles == 0 && managerOptions.GroupKindConcurrency[hostGroupKind] == 0 {
		hostOptions.MaxConcurrentReconciles = 2
	}
	return ctrl.NewControllerManagedBy(mgr).
		WithOptions(hostOptions).
		For(&runtimev1alpha1.Host{}, builder.WithPredicates(predicate.GenerationChangedPredicate{})).
		Named("workload-host").
		Complete(r)
}

type hostStatusUpdater struct {
	bus    wasmbus.Bus
	client client.Client
	// operatorNamespace is the namespace every Host object is created in.
	operatorNamespace string
	// fleet is stamped on every heartbeat that arrives.
	fleet *fleetWitness
	// retiredHosts rejects late heartbeats after a Pod disappears.
	retiredHosts *RetiredHosts
}

func (h *hostStatusUpdater) Start(ctx context.Context) error {
	subscription, err := h.bus.Subscribe("runtime.operator.heartbeat.>", 100)
	if err != nil {
		return err
	}

	log := ctrl.LoggerFrom(ctx).WithName("host-status-updater")

	subscription.Handle(func(msg *wasmbus.Message) {
		h.handleHeartbeat(ctx, log, msg.Subject, msg.Data)
	})

	<-ctx.Done()
	return subscription.Drain()
}

// handleHeartbeat accepts only the subject's host ID and refuses name takeovers.
// Existing Host writes require the resource version checked by the handler.
func (h *hostStatusUpdater) handleHeartbeat(ctx context.Context, log logr.Logger, subject string, data []byte) {
	var req runtimev2.HostHeartbeat
	if err := protojson.Unmarshal(data, &req); err != nil {
		log.Error(err, "failed to decode heartbeat message")
		return
	}

	hostID, ok := strings.CutPrefix(subject, "runtime.operator.heartbeat.")
	if !ok || hostID == "" || strings.ContainsAny(hostID, ".*> \t\r\n") || req.Id != hostID {
		log.Error(fmt.Errorf("heartbeat ID does not match its subject"), "rejecting heartbeat", "subject", subject, "hostID", req.Id)
		return
	}

	// Stamped before the bookkeeping below, which can fail on its own: arrival
	// is what proves the path from hosts to the operator works.
	h.fleet.heard(time.Now())
	var startedAt time.Time
	if req.GetStartedAt() != nil {
		startedAt = req.GetStartedAt().AsTime()
	}
	if h.retiredHosts.contains(req.Id) || h.retiredHosts.fromRetiredPod(req.Hostname, startedAt) {
		return
	}

	// Every Host object lives in the operator's own namespace. Tenant
	// attribution is recorded on the Host's Environment field,
	// populated verbatim from req.Environment — the heartbeat is the
	// source of truth and is not validated against cluster state.
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
	// Read the current state from the cache to see if the values in the heartbeat changed.
	existing := &runtimev1alpha1.Host{}
	getErr := h.client.Get(ctx,
		types.NamespacedName{Name: req.FriendlyName, Namespace: h.operatorNamespace},
		existing)
	if getErr != nil && !apierrors.IsNotFound(getErr) {
		log.Error(getErr, "failed to read Host resource", "host", req.FriendlyName, "hostID", req.Id)
		return
	}

	if getErr == nil && existing.HostID != hostID {
		log.Error(fmt.Errorf("host name belongs to another host ID"), "rejecting heartbeat", "host", req.FriendlyName, "hostID", hostID)
		return
	}

	// A host keeps heartbeating while its Pod drains, after the Pod's Host was
	// deleted. Recreating the Host would offer the draining host new Workloads.
	if apierrors.IsNotFound(getErr) {
		draining, err := hostPodDraining(ctx, h.client, req.Hostname)
		if err != nil {
			log.Error(err, "failed to check for a draining host Pod", "host", req.FriendlyName, "hostID", req.Id)
		}
		if draining {
			return
		}
	}

	switch {
	case apierrors.IsNotFound(getErr):
		// Create must fail if another host claimed this name after the read.
		if err := h.client.Create(ctx, host, client.FieldOwner("host-status-updater")); err != nil {
			log.Error(err, "failed to create Host resource", "host", req.FriendlyName, "hostID", hostID)
			return
		}
	case hostSpecChanged(existing, host):
		// A merge patch from the read object clears dropped labels and fields,
		// and its resource version fails the patch if the Host changed since.
		next := existing.DeepCopy()
		next.Labels = host.Labels
		next.Hostname = host.Hostname
		next.HTTPPort = host.HTTPPort
		next.Environment = host.Environment
		if err := h.client.Patch(ctx, next,
			client.MergeFromWithOptions(existing, client.MergeFromWithOptimisticLock{}),
			client.FieldOwner("host-status-updater")); err != nil {
			log.Error(err, "failed to patch Host resource", "host", req.FriendlyName, "hostID", hostID)
			return
		}
		host = next
	default:
		host = existing
	}

	// Patch only LastSeen on the status subresource. The measured system/OS
	// fields are written by reconcileReporting and must not be touched here.
	statusPatch, err := hostStatusPatch(host.ResourceVersion)
	if err != nil {
		log.Error(err, "failed to build Host status patch", "host", req.FriendlyName, "hostID", req.Id)
		return
	}
	if err := h.client.Status().Patch(ctx, host, client.RawPatch(types.MergePatchType, statusPatch)); err != nil {
		log.Error(err, "failed to update Host status", "host", req.FriendlyName, "hostID", req.Id)
	}
}

// hostSpecChanged returns true if any of the stored metadata
// spec fields differs from what the heartbeat returns
func hostSpecChanged(existing, next *runtimev1alpha1.Host) bool {
	return existing.HostID != next.HostID ||
		existing.Hostname != next.Hostname ||
		existing.HTTPPort != next.HTTPPort ||
		existing.Environment != next.Environment ||
		!maps.Equal(existing.Labels, next.Labels)
}

// hostStatusPatch builds a JSON merge patch for the Host status subresource
// that always refreshes LastSeen.
func hostStatusPatch(resourceVersion string) ([]byte, error) {
	patch := map[string]any{"status": map[string]any{"lastSeen": metav1.Now()}}
	if resourceVersion != "" {
		patch["metadata"] = map[string]any{"resourceVersion": resourceVersion}
	}
	return json.Marshal(patch)
}
