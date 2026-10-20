package runtime

import (
	"context"
	"fmt"
	"math/rand/v2"
	"strings"
	"sync"
	"time"

	"golang.org/x/time/rate"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/client-go/tools/events"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
)

const (
	workloadReconcileInterval = 30 * time.Second
	workloadStartTimeout      = 60 * time.Second
	workloadStatusTimeout     = 5 * time.Second
	workloadStopTimeout       = 30 * time.Second
	// workloadEvictionStopTimeout bounds how long one eviction holds the
	// controller's worker on a host that is not answering.
	workloadEvictionStopTimeout   = 5 * time.Second
	workloadFinalizerName         = "runtime.wasmcloud.dev/workload-finalizer"
	workloadSchedulableHostsIndex = "spec.isSchedulable"
	// hostEnvironmentIndex indexes Hosts by their Environment field for
	// O(1) lookup when scheduling enforces tenant isolation. The
	// Environment value is whatever the host self-reported in its
	// heartbeat (typically a namespace); the Host object itself always
	// lives in the operator's namespace.
	hostEnvironmentIndex = "spec.environment"
)

// WorkloadReconciler reconciles a Workload object
type WorkloadReconciler struct {
	client.Client
	Scheme   *runtime.Scheme
	Bus      wasmbus.Bus
	Recorder events.EventRecorder
	// OperatorNamespace is the namespace every Host CRD lives in. The
	// scheduler always lists Hosts in this namespace; tenant boundaries
	// are enforced via the Host.Environment field, not via Host.Namespace.
	OperatorNamespace string
	// AllowSharedHosts controls whether a Workload may schedule onto a Host
	// whose Environment differs from the Workload's own namespace (via
	// Spec.Environment). When false, cross-tenant scheduling is rejected
	// and a Warning Event is recorded on the Workload.
	AllowSharedHosts bool
	// EvictionLimiter paces stopping workloads placed outside the
	// Environment they may run in. Nil stops them all as they are found.
	EvictionLimiter *rate.Limiter
	// stalledHosts and pinnedWaits are this process's memory only. A restart
	// or a new leader starts them empty: each stalled host gets one fresh stop
	// attempt, and each pinned wait still under way is recorded once more.
	stalledHosts stalledHosts
	// pinnedWaits maps the UID of a Workload waiting for its pinned host to
	// register to that host's ID, once the wait has been recorded.
	pinnedWaits sync.Map
	reconciler  condition.AnyConditionedReconciler
}

// stalledHosts remembers hosts that did not answer an eviction's stop, so the
// workloads still to be evicted from them wait rather than each spending a
// stop timeout. The wait doubles with each consecutive failure, from one
// eviction stop timeout up to a reconcile interval, so a single blip costs
// the rest of the host's evictions only seconds. A host whose stall ended a
// reconcile interval ago is forgotten, so the map holds only recent failures.
type stalledHosts struct {
	mu     sync.Mutex
	stalls map[string]hostStall
}

type hostStall struct {
	failures int
	until    time.Time
}

func (s *stalledHosts) failed(hostID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.stalls == nil {
		s.stalls = make(map[string]hostStall)
	}
	now := time.Now()
	for id, stall := range s.stalls {
		if now.Sub(stall.until) > workloadReconcileInterval {
			delete(s.stalls, id)
		}
	}
	stall := s.stalls[hostID]
	stall.failures++
	wait := workloadEvictionStopTimeout << min(stall.failures-1, 8)
	stall.until = now.Add(min(wait, workloadReconcileInterval))
	s.stalls[hostID] = stall
}

func (s *stalledHosts) answered(hostID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.stalls, hostID)
}

func (s *stalledHosts) stalled(hostID string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return time.Now().Before(s.stalls[hostID].until)
}

// NewEvictionLimiter allows ten evictions a second, the rate at which a
// cluster that turns allowSharedHosts off moves its cross-tenant workloads.
func NewEvictionLimiter() *rate.Limiter {
	return rate.NewLimiter(10, 10)
}

func (r *WorkloadReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	return r.reconciler.Reconcile(ctx, req)
}

func (r *WorkloadReconciler) reconcileConfig(ctx context.Context, workload *runtimev1alpha1.Workload) error {
	// If the workload is already assigned to a host, there's nothing to do.
	// We only need the config during workload placement.
	if workload.Status.HostID != "" {
		return nil
	}

	if workload.Spec.Service != nil {
		if workload.Spec.Service.LocalResources != nil {
			_, err := MaterializeConfigLayer(ctx, r.Client, workload.Namespace, workload.Spec.Service.LocalResources.Environment)
			if err != nil {
				return fmt.Errorf("materializing local resources config: %w", err)
			}
		}
	}

	for _, c := range workload.Spec.Components {
		if c.LocalResources != nil {
			if c.LocalResources.Environment != nil {
				_, err := MaterializeConfigLayer(ctx, r.Client, workload.Namespace, c.LocalResources.Environment)
				if err != nil {
					return fmt.Errorf("materializing local resources config for component %q: %w", c.Name, err)
				}
			}
		}
	}

	return nil
}

func (r *WorkloadReconciler) reconcileHostSelection(ctx context.Context, workload *runtimev1alpha1.Workload) error {
	if workload.Status.HostID != "" {
		return r.evictOutsideEnvironment(ctx, workload)
	}

	if !workload.Status.AllTrue(runtimev1alpha1.WorkloadConditionConfig) {
		return condition.ErrStatusUnknown(fmt.Errorf("waiting for config"))
	}

	condition.ForceStatusUpdate(ctx)
	// A read that predates the recorded HostID would pick a second host and
	// overwrite the first after the start already went to it.
	condition.RequireOptimisticLock(ctx)
	if workload.Spec.HostID != "" {
		environment, err := r.allowedEnvironment(workload)
		if err != nil {
			return r.denyEnvironment(workload, "Reject", err)
		}
		host, err := r.lookupHostByID(ctx, workload.Spec.HostID)
		if environment != "" {
			// Isolation needs the pinned host's Environment, so a host that
			// cannot be read or is not registered yet is waited on.
			switch {
			case err != nil:
				return err
			case host == nil:
				waiting := condition.ErrStatusUnknown(fmt.Errorf(
					"pinned host %s is not registered, so its Environment cannot be checked", workload.Spec.HostID))
				// Once when the wait begins, not on every pass of it, nor again
				// after a pass that failed some other way.
				if waitedOn, _ := r.pinnedWaits.Swap(workload.UID, workload.Spec.HostID); waitedOn != workload.Spec.HostID && r.Recorder != nil {
					r.Recorder.Eventf(workload, nil, corev1.EventTypeWarning,
						"PinnedHostNotRegistered", "Wait",
						"Pinned host %q has no Host object; placement waits for one so its Environment can be checked",
						workload.Spec.HostID)
				}
				return waiting
			case host.Environment != environment:
				r.pinnedWaits.Delete(workload.UID)
				return r.denyHost(workload, host, environment, "Reject")
			}
			r.pinnedWaits.Delete(workload.UID)
		}
		if err == nil && host != nil && !host.DeletionTimestamp.IsZero() {
			return condition.ErrStatusUnknown(fmt.Errorf("pinned host %s is being deleted", workload.Spec.HostID))
		}
		workload.Status.HostID = workload.Spec.HostID
		// Best-effort lookup of the pinned host's Environment so the
		// ENVIRONMENT status column reflects the host's tenant. A miss
		// here just leaves the field empty; placement will surface the
		// real failure if the HostID is invalid.
		if err == nil && host != nil {
			workload.Status.Environment = host.Environment
		}
		return condition.ErrSkipReconciliation()
	}

	r.pinnedWaits.Delete(workload.UID)
	selectedHost, err := r.findFreeHost(ctx, workload)
	if err != nil {
		return err
	}

	workload.Status.HostID = selectedHost.HostID
	workload.Status.Environment = selectedHost.Environment
	return condition.ErrSkipReconciliation()
}

func (r *WorkloadReconciler) findFreeHost(ctx context.Context, workload *runtimev1alpha1.Workload) (*runtimev1alpha1.Host, error) {
	hostList := runtimev1alpha1.HostList{}

	// Every Host object lives in the operator's namespace. Tenant
	// isolation is enforced by an indexed match on Host.Environment, not by
	// Host.Namespace.
	listOpts := []client.ListOption{
		client.InNamespace(r.OperatorNamespace),
		client.MatchingLabels(workload.Spec.HostSelector),
		client.MatchingFields{
			workloadSchedulableHostsIndex: string(condition.ConditionTrue),
		},
	}

	environmentFilter, err := r.allowedEnvironment(workload)
	if err != nil {
		return nil, r.denyEnvironment(workload, "Reject", err)
	}
	if environmentFilter != "" {
		listOpts = append(listOpts, client.MatchingFields{
			hostEnvironmentIndex: environmentFilter,
		})
	}

	if err := r.List(ctx, &hostList, listOpts...); err != nil {
		return nil, err
	}

	// Shuffle the host list
	rand.Shuffle(len(hostList.Items), func(i, j int) {
		hostList.Items[i], hostList.Items[j] = hostList.Items[j], hostList.Items[i]
	})
	for i := range hostList.Items {
		host := &hostList.Items[i]
		if isSchedulable(host) {
			return host, nil
		}
	}

	return nil, fmt.Errorf("no suitable host found")
}

// denyHost records that host's Environment is outside the one the workload may
// be placed in, and returns the error for its condition.
func (r *WorkloadReconciler) denyHost(workload *runtimev1alpha1.Workload, host *runtimev1alpha1.Host, environment, action string) error {
	if r.Recorder != nil {
		r.Recorder.Eventf(workload, nil, corev1.EventTypeWarning,
			"CrossEnvironmentSchedulingDenied", action,
			"Host %q is in Environment %q; this workload in %q may only be placed in %q",
			host.HostID, host.Environment, workload.Namespace, environment,
		)
	}
	return fmt.Errorf("host %s is in Environment %q, but the workload may only be placed in %q",
		host.HostID, host.Environment, environment)
}

// evictOutsideEnvironment stops a placed workload whose host the Environment
// rule does not admit, such as one placed before allowSharedHosts was turned
// off, and clears its placement so host selection runs again under the rule.
//
// Stops are paced by EvictionLimiter, and a heartbeating host that did not
// answer one is skipped for a while (see stalledHosts), so a mass eviction
// neither restarts every workload at once nor holds the controller on a host
// that is not answering.
func (r *WorkloadReconciler) evictOutsideEnvironment(ctx context.Context, workload *runtimev1alpha1.Workload) error {
	hostID := workload.Status.HostID
	environment, denied := r.allowedEnvironment(workload)
	if denied == nil && environment == "" {
		return nil
	}
	host, err := r.lookupHostByID(ctx, hostID)
	if err != nil {
		return err
	}
	// A host that is gone is left to placement and sync.
	if denied == nil && (host == nil || host.Environment == environment) {
		return nil
	}

	// Unknown, not an error: the reconcile interval is the retry, rather than
	// a requeue a second later for every workload waiting its turn.
	//
	// Eviction drops a workload it could not stop only from a host known to
	// have failed. One that is merely not reporting, as after a missed
	// heartbeat, may still be running it, and a workload dropped there would
	// keep running on the wrong tenant's host with nothing tracking it.
	failed := hostFailed(host)
	stalled := r.stalledHosts.stalled(hostID)
	if stalled && !failed {
		return condition.ErrStatusUnknown(fmt.Errorf(
			"host %s is outside the allowed Environment and did not answer a stop; retrying", hostID))
	}
	if r.EvictionLimiter != nil && !r.EvictionLimiter.Allow() {
		return condition.ErrStatusUnknown(fmt.Errorf(
			"host %s is outside the allowed Environment; waiting to stop the workload there", hostID))
	}
	switch {
	case stalled:
		// Failed, and it did not answer the last stop: stopOnHost would only
		// spend its timeout to give up on it again.
		ctrl.LoggerFrom(ctx).Info("dropping a workload from a failed host that did not answer a stop",
			"hostID", hostID, "workloadID", string(workload.GetUID()))
	default:
		answered, err := r.stopOnHost(ctx, workload, workloadEvictionStopTimeout, hostFailed)
		if err != nil || !answered {
			r.stalledHosts.failed(hostID)
		} else {
			r.stalledHosts.answered(hostID)
		}
		if err != nil {
			return err
		}
	}

	// Recorded once the stop has gone through, not on every pass before it.
	var reason error
	if denied == nil {
		reason = fmt.Errorf("stopped: %w", r.denyHost(workload, host, environment, "Evict"))
	} else {
		reason = fmt.Errorf("stopped on host %s: %w", hostID, r.denyEnvironment(workload, "Evict", denied))
	}
	workload.Status.HostID = ""
	workload.Status.WorkloadID = ""
	workload.Status.Environment = ""
	// The loop stops at this condition's change, so the ones after it would
	// otherwise still say the workload is placed, synced and ready.
	for _, ct := range []condition.ConditionType{
		runtimev1alpha1.WorkloadConditionPlacement,
		runtimev1alpha1.WorkloadConditionSync,
		condition.TypeReady,
	} {
		workload.Status.SetConditions(condition.UnknownCondition(ct, "Evicted", reason.Error()))
	}
	condition.ForceStatusUpdate(ctx)
	condition.RequireOptimisticLock(ctx)
	return condition.ErrStatusUnknown(reason)
}

// denyEnvironment records that the workload's own spec.environment is refused,
// and returns err for its condition.
func (r *WorkloadReconciler) denyEnvironment(workload *runtimev1alpha1.Workload, action string, err error) error {
	if r.Recorder != nil {
		r.Recorder.Eventf(workload, nil, corev1.EventTypeWarning,
			"CrossEnvironmentSchedulingDenied", action,
			"Environment %q is outside %q and operator has allowSharedHosts=false",
			workload.Spec.Environment, workload.Namespace,
		)
	}
	return err
}

// allowedEnvironment resolves the Host.Environment a workload may be placed in,
// per the AllowSharedHosts × Spec.Environment matrix in namespace-host.md, for
// both a chosen and a pinned host. Empty allows any: the default mode
// (AllowSharedHosts=true, Environment unset) keeps Cluster-scope scheduling.
func (r *WorkloadReconciler) allowedEnvironment(workload *runtimev1alpha1.Workload) (string, error) {
	switch {
	case workload.Spec.Environment != "" && workload.Spec.Environment != workload.Namespace:
		// Explicit cross-tenant target.
		if !r.AllowSharedHosts {
			return "", fmt.Errorf(
				"workload targets Environment %q but operator has allowSharedHosts=false",
				workload.Spec.Environment,
			)
		}
		return workload.Spec.Environment, nil
	case workload.Spec.Environment != "":
		// Environment explicitly set to the workload's own namespace.
		return workload.Namespace, nil
	case !r.AllowSharedHosts:
		// No Environment, sharing disabled — lock to the workload's namespace.
		return workload.Namespace, nil
	default:
		return "", nil
	}
}

// isSchedulable reports whether new workloads may be placed on host. A Host
// being deleted stays Ready until it is gone, but its finalizer may already
// have listed the workloads it cleans up, so one placed now would be missed.
func isSchedulable(host *runtimev1alpha1.Host) bool {
	return host.DeletionTimestamp.IsZero() && host.Status.IsAvailable()
}

func schedulableHostIndexValue(rawObj client.Object) []string {
	if host, ok := rawObj.(*runtimev1alpha1.Host); ok && isSchedulable(host) {
		return []string{string(condition.ConditionTrue)}
	}
	return []string{}
}

// lookupHostByID finds a Host CRD by HostID. Hosts always live in the
// operator's namespace, so the lookup is namespace-scoped and indexed.
// Returns (nil, nil) when no host matches; an error only on List failure.
func (r *WorkloadReconciler) lookupHostByID(ctx context.Context, hostID string) (*runtimev1alpha1.Host, error) {
	var hosts runtimev1alpha1.HostList
	if err := r.List(ctx, &hosts,
		client.InNamespace(r.OperatorNamespace),
		client.MatchingFields{hostIDIndex: hostID},
	); err != nil {
		return nil, err
	}
	if len(hosts.Items) == 0 {
		return nil, nil
	}
	return &hosts.Items[0], nil
}

// materializeLocalResources converts a spec-level LocalResources into the
// runtimev2 equivalent, materializing config layers and volume mounts.
func materializeLocalResources(ctx context.Context, c client.Client, namespace string, spec *runtimev1alpha1.LocalResources, label string) (*runtimev2.LocalResources, error) {
	lr := &runtimev2.LocalResources{}
	if spec == nil {
		return lr, nil
	}

	lr.AllowedHosts = spec.AllowedHosts
	lr.AllowedIpNameLookups = spec.AllowedIPNameLookups
	lr.AllowedHostLoopbackPorts = spec.AllowedHostLoopbackPorts
	lr.Config = spec.Config

	if spec.Environment != nil {
		env, err := MaterializeConfigLayer(ctx, c, namespace, spec.Environment)
		if err != nil {
			return nil, fmt.Errorf("materializing local resources config for %s: %w", label, err)
		}
		lr.Environment = env
	}

	if spec.VolumeMounts != nil {
		lr.VolumeMounts = make([]*runtimev2.VolumeMount, 0, len(spec.VolumeMounts))
		for _, vm := range spec.VolumeMounts {
			lr.VolumeMounts = append(lr.VolumeMounts, &runtimev2.VolumeMount{
				Name:      vm.Name,
				MountPath: vm.MountPath,
				ReadOnly:  vm.ReadOnly,
			})
		}
	}

	return lr, nil
}

// injectServiceDNSAliases adds host-aliases to wasi:http/incoming-handler
// HostInterfaces so the wash-runtime DynamicRouter accepts requests arriving
// via Kubernetes Service DNS.
func injectServiceDNSAliases(hostInterfaces []*runtimev2.WitInterface, svcName, namespace string) {
	aliases := strings.Join([]string{
		fmt.Sprintf("%s.%s", svcName, namespace),
		fmt.Sprintf("%s.%s.svc", svcName, namespace),
	}, ",")

	for i, hi := range hostInterfaces {
		if hi.Namespace != "wasi" || hi.Package != "http" {
			continue
		}

		for _, iface := range hi.Interfaces {
			if iface == "incoming-handler" {
				if hi.Config == nil {
					hi.Config = make(map[string]string)
				}
				hi.Config["host-aliases"] = aliases
				hostInterfaces[i] = hi
			}
		}
	}
}

func (r *WorkloadReconciler) reconcilePlacement(ctx context.Context, workload *runtimev1alpha1.Workload) error {
	if workload.Status.HostID == "" {
		return condition.ErrStatusUnknown(fmt.Errorf("waiting for Host Selection"))
	}
	// don't replace
	if workload.Status.WorkloadID != "" {
		return nil
	}

	// The host may have gone, or started deleting, after it was chosen. Its
	// finalizer has listed its Workloads already, so one started there now is
	// missed. A pinned host that is not registered yet is still tried.
	host, err := r.lookupHostByID(ctx, workload.Status.HostID)
	if err != nil {
		return err
	}
	goingAway := host != nil && !host.DeletionTimestamp.IsZero()
	if goingAway || (host == nil && workload.Spec.HostID == "") {
		hostID := workload.Status.HostID
		if workload.Spec.HostID == "" {
			workload.Status.HostID = ""
			workload.Status.Environment = ""
			condition.ForceStatusUpdate(ctx)
			condition.RequireOptimisticLock(ctx)
		}
		return condition.ErrStatusUnknown(fmt.Errorf("host %s is gone or being deleted", hostID))
	}

	volumes := make([]*runtimev2.Volume, 0, len(workload.Spec.Volumes))
	for _, v := range workload.Spec.Volumes {
		vol := &runtimev2.Volume{
			Name: v.Name,
		}
		switch {
		case v.EphemeralVolume != nil:
			vol.VolumeType = &runtimev2.Volume_EmptyDir{}
		case v.HostPathVolume != nil:
			vol.VolumeType = &runtimev2.Volume_HostPath{
				HostPath: &runtimev2.HostPathVolume{
					LocalPath: v.HostPathVolume.Path,
				},
			}
		}

		volumes = append(volumes, vol)
	}

	witWorld := &runtimev2.WitWorld{
		HostInterfaces: make([]*runtimev2.WitInterface, 0, len(workload.Spec.HostInterfaces)),
		Components:     make([]*runtimev2.Component, 0, len(workload.Spec.Components)),
	}
	for _, hi := range workload.Spec.HostInterfaces {
		hiConfig, err := MaterializeConfigLayer(ctx, r.Client, workload.Namespace, &hi.ConfigLayer)
		if err != nil {
			return fmt.Errorf("materializing host interface config for %s/%s: %w", hi.Namespace, hi.Package, err)
		}
		witWorld.HostInterfaces = append(witWorld.HostInterfaces, &runtimev2.WitInterface{
			Namespace:  hi.Namespace,
			Package:    hi.Package,
			Version:    hi.Version,
			Interfaces: hi.Interfaces,
			Config:     hiConfig,
			Name:       hi.Name,
		})
	}

	for _, c := range workload.Spec.Components {
		localResources, err := materializeLocalResources(ctx, r.Client, workload.Namespace, c.LocalResources, fmt.Sprintf("component %q", c.Name))
		if err != nil {
			return err
		}

		var imagePullSecret *runtimev2.ImagePullSecret
		if c.ImagePullSecret != nil {
			imagePullSecret, err = MaterializeImagePullSecret(ctx, r.Client, workload.Namespace, c.ImagePullSecret.Name, c.Image)
			if err != nil {
				return fmt.Errorf("materializing image pull secret for component %q: %w", c.Name, err)
			}
		}

		witWorld.Components = append(witWorld.Components, &runtimev2.Component{
			Name:                 c.Name,
			Image:                c.Image,
			ImagePullSecret:      imagePullSecret,
			ImagePullPolicy:      translatePullPolicy(c.ImagePullPolicy),
			PoolSize:             c.PoolSize,
			MaxInvocations:       c.MaxInvocations,
			MaxConcurrency:       c.MaxConcurrency,
			ReclaimWindowSeconds: c.ReclaimWindowSeconds,
			ReclaimMinInstances:  c.ReclaimMinInstances,
			LocalResources:       localResources,
		})
	}

	if workload.Spec.Kubernetes != nil && workload.Spec.Kubernetes.Service != nil {
		injectServiceDNSAliases(witWorld.HostInterfaces, workload.Spec.Kubernetes.Service.Name, workload.Namespace)
	}

	var service *runtimev2.Service
	if s := workload.Spec.Service; s != nil {
		localResources, err := materializeLocalResources(ctx, r.Client, workload.Namespace, s.LocalResources, "service")
		if err != nil {
			return err
		}

		var imagePullSecret *runtimev2.ImagePullSecret
		if s.ImagePullSecret != nil {
			imagePullSecret, err = MaterializeImagePullSecret(ctx, r.Client, workload.Namespace, s.ImagePullSecret.Name, s.Image)
			if err != nil {
				return fmt.Errorf("materializing image pull secret for service: %w", err)
			}
		}

		service = &runtimev2.Service{
			Image:           s.Image,
			ImagePullSecret: imagePullSecret,
			ImagePullPolicy: translatePullPolicy(s.ImagePullPolicy),
			LocalResources:  localResources,
			MaxRestarts:     uint64(s.MaxRestarts),
		}
	}

	req := &runtimev2.WorkloadStartRequest{
		WorkloadId: string(workload.GetUID()),
		Workload: &runtimev2.Workload{
			Namespace:   workload.Namespace,
			Name:        workload.Name,
			Annotations: workload.GetAnnotations(),
			WitWorld:    witWorld,
			Volumes:     volumes,
			Service:     service,
		},
	}

	client := NewWashHostClient(r.Bus, workload.Status.HostID)
	ctx, cancel := context.WithTimeout(ctx, workloadStartTimeout)
	defer cancel()

	resp, err := client.Start(ctx, req)
	if err != nil {
		return err
	}

	// Set the WorkloadID in the status
	workload.Status.WorkloadID = resp.WorkloadStatus.WorkloadId
	condition.ForceStatusUpdate(ctx)
	return condition.ErrSkipReconciliation()
}

func (r *WorkloadReconciler) reconcileSync(ctx context.Context, workload *runtimev1alpha1.Workload) error {
	if !workload.Status.AllTrue(runtimev1alpha1.WorkloadConditionPlacement) {
		return condition.ErrStatusUnknown(fmt.Errorf("workload is not placed yet"))
	}

	// A draining host still answers status, so the Host is the only sign it
	// is going away. Failing sync lets the ReplicaSet replace the workload.
	host, err := r.lookupHostByID(ctx, workload.Status.HostID)
	if err != nil {
		return err
	}
	if host == nil {
		return fmt.Errorf("host %s no longer exists", workload.Status.HostID)
	}
	if !host.DeletionTimestamp.IsZero() {
		return fmt.Errorf("host %s is being deleted", workload.Status.HostID)
	}

	client := NewWashHostClient(r.Bus, workload.Status.HostID)
	req := &runtimev2.WorkloadStatusRequest{
		WorkloadId: workload.Status.WorkloadID,
	}

	ctx, cancel := context.WithTimeout(ctx, workloadStatusTimeout)
	defer cancel()

	resp, err := client.Status(ctx, req)
	if err != nil {
		return err
	}
	if resp.WorkloadStatus.WorkloadState == runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING ||
		resp.WorkloadStatus.WorkloadState == runtimev2.WorkloadState_WORKLOAD_STATE_COMPLETED {
		return nil
	}

	// The host's own reason, not just the state name. `WORKLOAD_STATE_ERROR`
	// on its own is what a `kubectl describe` used to show for a workload the
	// host had already diagnosed precisely — a component whose minimum linear
	// memory exceeds `--default-heap-memory`, say, where the host log names
	// the requirement, the flag and the value to raise it to. This condition
	// message is the only path that reason has to somebody who is not reading
	// host logs, so it carries it.
	// Only an errored workload carries a message; every other state is named
	// by WorkloadState alone, so this does not render the state twice.
	if msg := resp.WorkloadStatus.Message; msg != "" {
		return fmt.Errorf("workload is not operational: %s: %s",
			resp.WorkloadStatus.WorkloadState.String(), msg)
	}
	return fmt.Errorf("workload is not operational: %s", resp.WorkloadStatus.WorkloadState.String())
}

func (r *WorkloadReconciler) reconcileReady(_ context.Context, workload *runtimev1alpha1.Workload) error {
	if !workload.Status.AllTrue(runtimev1alpha1.WorkloadConditionPlacement, runtimev1alpha1.WorkloadConditionSync) {
		return fmt.Errorf("workload is not placed or not synced")
	}

	return nil
}

func (r *WorkloadReconciler) finalize(ctx context.Context, workload *runtimev1alpha1.Workload) error {
	r.pinnedWaits.Delete(workload.UID)
	// Don't gate on Placement: it is recorded only after the start returns, so a
	// Workload deleted mid-start would skip the stop and stay running on the host.
	// HostID is set before any start is sent.
	if workload.Status.HostID == "" {
		// nothing to do, no start was ever sent
		return nil
	}
	// Deletion gives up sooner than eviction, on any host not heartbeating:
	// waiting for its Host to be reaped could block the deletion for good.
	_, err := r.stopOnHost(ctx, workload, workloadStopTimeout, hostUnavailable)
	return err
}

// hostUnavailable reports whether host is gone or not heartbeating.
func hostUnavailable(host *runtimev1alpha1.Host) bool {
	return host == nil || !host.Status.IsAvailable()
}

// hostFailed reports whether host is gone or known to have failed, rather
// than only not reporting for now.
func hostFailed(host *runtimev1alpha1.Host) bool {
	return host == nil || host.Status.GetCondition(condition.TypeReady).Status == condition.ConditionFalse
}

// stopOnHost stops the workload on the host it was placed on. A stop that
// fails is returned for a retry unless giveUp says the host is past waiting
// for; answered is false when it gave up on the host that way.
func (r *WorkloadReconciler) stopOnHost(ctx context.Context, workload *runtimev1alpha1.Workload, timeout time.Duration, giveUp func(*runtimev1alpha1.Host) bool) (answered bool, err error) {
	workloadID := string(workload.GetUID())

	client := NewWashHostClient(r.Bus, workload.Status.HostID)
	req := &runtimev2.WorkloadStopRequest{
		WorkloadId: workloadID,
	}

	stopCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	if _, err = client.Stop(stopCtx, req); err == nil {
		return true, nil
	}
	// A host that may be running the workload is asked again.
	host, lookupErr := r.lookupHostByID(ctx, workload.Status.HostID)
	if lookupErr != nil {
		return false, lookupErr
	}
	if !giveUp(host) {
		return false, fmt.Errorf("stop workload on host %s: %w", workload.Status.HostID, err)
	}
	ctrl.LoggerFrom(ctx).Error(err, "failed to stop workload on an unavailable host, dropping it",
		"hostID", workload.Status.HostID, "workloadID", workloadID)
	return false, nil
}

// SetupWithManager sets up the controller with the Manager.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloads,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloads/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloads/finalizers,verbs=update
// +kubebuilder:rbac:groups="",resources=configmaps,verbs=get;list;watch
// +kubebuilder:rbac:groups="",resources=secrets,verbs=get;list;watch
// +kubebuilder:rbac:groups="",resources=events,verbs=create;get;list;patch;update;watch

func (r *WorkloadReconciler) SetupWithManager(mgr ctrl.Manager) error {
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.Workload{},
		workloadReconcileInterval)
	reconciler.SetFinalizer(workloadFinalizerName, r.finalize)

	reconciler.SetCondition(runtimev1alpha1.WorkloadConditionConfig, r.reconcileConfig)
	reconciler.SetCondition(runtimev1alpha1.WorkloadConditionHostSelection, r.reconcileHostSelection)
	reconciler.SetCondition(runtimev1alpha1.WorkloadConditionPlacement, r.reconcilePlacement)
	reconciler.SetCondition(runtimev1alpha1.WorkloadConditionSync, r.reconcileSync)
	reconciler.SetCondition(condition.TypeReady, r.reconcileReady)

	r.reconciler = reconciler

	// Index Hosts by HostID for lookupHostByID. WorkloadRouteReconciler reads
	// it too.
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

	err := mgr.GetFieldIndexer().IndexField(context.Background(), &runtimev1alpha1.Host{}, workloadSchedulableHostsIndex, schedulableHostIndexValue)
	if err != nil {
		return err
	}

	// Index Hosts by their Environment field so findFreeHost can do an
	// O(matching) indexed list when AllowSharedHosts=false or
	// Spec.Environment pins scheduling to a specific tenant.
	if err := mgr.GetFieldIndexer().IndexField(
		context.Background(),
		&runtimev1alpha1.Host{},
		hostEnvironmentIndex,
		func(rawObj client.Object) []string {
			host, ok := rawObj.(*runtimev1alpha1.Host)
			if !ok || host.Environment == "" {
				return nil
			}
			return []string{host.Environment}
		},
	); err != nil {
		return err
	}

	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.Workload{}).
		Named("workload-replica").
		Complete(r)
}
