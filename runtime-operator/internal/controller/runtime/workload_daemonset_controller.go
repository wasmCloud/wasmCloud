package runtime

import (
	"context"
	"fmt"
	"maps"
	"slices"
	"strings"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/labels"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/handler"
	"sigs.k8s.io/controller-runtime/pkg/predicate"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	workloadDaemonSetReconcileInterval    = 1 * time.Minute
	workloadDaemonSetNameIndex            = "workload.daemonset.name"
	workloadDaemonSetGenerationAnnotation = "runtime.wasmcloud.dev/workload-daemonset-generation"
	// workloadDaemonSetNameAnnotation gives the host a workload name that is
	// stable across Hosts and rollouts.
	workloadDaemonSetNameAnnotation = "runtime.wasmcloud.dev/workload-daemonset"
	// workloadDaemonSetNameLabel is stamped onto the Workloads a DaemonSet
	// owns, with ownerLabelValue(name) as its value.
	workloadDaemonSetNameLabel = "runtime.wasmcloud.dev/workload-daemonset"
)

// WorkloadDaemonSetReconciler reconciles a WorkloadDaemonSet object
type WorkloadDaemonSetReconciler struct {
	client.Client
	// APIReader reads past the cache, for decisions a stale read would get wrong.
	APIReader client.Reader
	Scheme    *runtime.Scheme
	// OperatorNamespace is the namespace every Host CRD lives in.
	OperatorNamespace string
	// AllowSharedHosts has the same meaning as on WorkloadReconciler.
	AllowSharedHosts bool
	reconciler       condition.AnyConditionedReconciler
}

func (r *WorkloadDaemonSetReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	return r.reconciler.Reconcile(ctx, req)
}

func (r *WorkloadDaemonSetReconciler) reconcileArtifacts(ctx context.Context, daemonSet *runtimev1alpha1.WorkloadDaemonSet) error {
	return checkArtifactsPublished(ctx, r.Client, daemonSet.Namespace, daemonSet.Spec.Artifacts)
}

// daemonSetHost is a DaemonSet's Workloads on one Host, split by template.
type daemonSetHost struct {
	current  []*runtimev1alpha1.Workload
	outdated []*runtimev1alpha1.Workload
	// readySince is the Host's last Ready transition. Failure grace periods
	// start no earlier, so a host blip doesn't count against its instances.
	readySince time.Time
}

func (h *daemonSetHost) readyCurrent() int32 {
	ready := int32(0)
	for _, w := range h.current {
		if w.Status.IsAvailable() {
			ready++
		}
	}
	return ready
}

func (r *WorkloadDaemonSetReconciler) reconcileSchedule(ctx context.Context, daemonSet *runtimev1alpha1.WorkloadDaemonSet) error {
	// The garbage collector is removing or orphaning this DaemonSet's Workloads.
	if !daemonSet.DeletionTimestamp.IsZero() {
		return condition.ErrNoop()
	}

	if !daemonSet.Status.AllTrue(runtimev1alpha1.WorkloadDaemonSetConditionArtifact) {
		return condition.ErrStatusUnknown(fmt.Errorf("artifacts not ready"))
	}

	template := daemonSet.Spec.Template.DeepCopy()
	if err := resolveArtifacts(ctx, r.Client, daemonSet.Namespace, template, daemonSet.Spec.Artifacts); err != nil {
		return err
	}
	if daemonSet.Spec.Kubernetes != nil {
		template.Spec.Kubernetes = daemonSet.Spec.Kubernetes.DeepCopy()
	}
	templateHash := template.Hash()

	readySince, retained, err := r.matchingHosts(ctx, daemonSet.Namespace, &template.Spec)
	if err != nil {
		return err
	}
	desired := slices.Sorted(maps.Keys(readySince))

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, workloads,
		client.InNamespace(daemonSet.Namespace),
		client.MatchingFields{workloadDaemonSetNameIndex: daemonSet.Name},
	); err != nil {
		return err
	}
	orphans, err := r.adoptOrphans(ctx, daemonSet)
	if err != nil {
		return err
	}
	workloads.Items = append(workloads.Items, orphans...)

	hosts := make(map[string]*daemonSetHost, len(desired))
	for hostID, since := range readySince {
		hosts[hostID] = &daemonSetHost{readySince: since}
	}
	// Names still in use, including by Workloads being deleted.
	taken := make(map[string]bool, len(workloads.Items))
	for i := range workloads.Items {
		workload := &workloads.Items[i]
		taken[workload.Name] = true
		if workload.DeletionTimestamp != nil {
			continue
		}
		host, ok := hosts[workload.Spec.HostID]
		if !ok {
			if retained[workload.Spec.HostID] {
				continue
			}
			// The host left or stopped matching. Its instances go with it and
			// nothing is started elsewhere in their place.
			if err := r.Delete(ctx, workload); client.IgnoreNotFound(err) != nil {
				return err
			}
			continue
		}
		if workload.Annotations[workloadDaemonSetGenerationAnnotation] == templateHash {
			host.current = append(host.current, workload)
		} else {
			host.outdated = append(host.outdated, workload)
		}
	}

	perHost := int32(1)
	if daemonSet.Spec.ReplicasPerHost != nil {
		perHost = *daemonSet.Spec.ReplicasPerHost
	}
	maxConcurrentHosts := 1
	if daemonSet.Spec.MaxConcurrentHosts != nil {
		maxConcurrentHosts = int(*daemonSet.Spec.MaxConcurrentHosts)
	}
	recreate := daemonSet.Spec.DeployPolicy == runtimev1alpha1.WorkloadDeployPolicyRecreate

	// Report what this pass leaves behind, not what it started from.
	defer r.setScheduleStatus(ctx, daemonSet, desired, hosts, perHost)

	// A host is mid-update while it runs both templates and the new
	// instances are not all up yet; one finishing this pass frees its slot.
	updating := 0
	for _, hostID := range desired {
		if h := hosts[hostID]; len(h.outdated) > 0 && len(h.current) > 0 && h.readyCurrent() < perHost {
			updating++
		}
	}

	for _, hostID := range desired {
		host := hosts[hostID]

		if len(host.outdated) > 0 {
			switch {
			case recreate:
				if err := r.deleteAll(ctx, host.outdated); err != nil {
					return err
				}
				host.outdated = nil
			case len(host.current) == 0:
				// The host keeps its old instances until its turn to update.
				if updating >= maxConcurrentHosts {
					continue
				}
				updating++
			}
		}

		if err := r.syncHost(ctx, daemonSet, template, templateHash, hostID, host, perHost, taken); err != nil {
			return err
		}
	}

	return nil
}

// syncHost brings one Host to perHost instances of the current template,
// retiring the outdated ones once the new ones there are up.
func (r *WorkloadDaemonSetReconciler) syncHost(
	ctx context.Context,
	daemonSet *runtimev1alpha1.WorkloadDaemonSet,
	template *runtimev1alpha1.WorkloadReplicaTemplate,
	templateHash string,
	hostID string,
	host *daemonSetHost,
	perHost int32,
	taken map[string]bool,
) error {
	// Fill the lowest free slots. A create the cache hasn't seen yet comes
	// back AlreadyExists instead of starting a second copy.
	for slot := 0; len(host.current) < int(perHost); slot++ {
		name := daemonSetWorkloadName(daemonSet.Name, hostID, templateHash, slot)
		if taken[name] {
			continue
		}
		workload, err := r.createWorkload(ctx, daemonSet, template, templateHash, hostID, name)
		if err != nil {
			return err
		}
		taken[name] = true
		host.current = append(host.current, workload)
	}

	if excess := len(host.current) - int(perHost); excess > 0 {
		// Remove instances that are not available first.
		slices.SortStableFunc(host.current, func(a, b *runtimev1alpha1.Workload) int {
			switch {
			case a.Status.IsAvailable() == b.Status.IsAvailable():
				return 0
			case a.Status.IsAvailable():
				return 1
			default:
				return -1
			}
		})
		if err := r.deleteAll(ctx, host.current[:excess]); err != nil {
			return err
		}
		host.current = host.current[excess:]
	}

	// Old instances stop only once the new ones on the same host are up.
	if len(host.outdated) > 0 && len(host.current) >= int(perHost) && host.readyCurrent() >= perHost {
		if err := r.deleteAll(ctx, host.outdated); err != nil {
			return err
		}
		host.outdated = nil
	}

	// A failed instance is replaced on its own host on the next pass.
	running := host.current[:0]
	for _, workload := range host.current {
		if !isFailedPastGracePeriod(workload, host.readySince) {
			running = append(running, workload)
			continue
		}
		if err := r.Delete(ctx, workload); client.IgnoreNotFound(err) != nil {
			return err
		}
	}
	host.current = running

	return nil
}

// daemonSetWorkloadName names the Workload in one slot on one Host, so a
// retried create collides with the first instead of adding another.
func daemonSetWorkloadName(daemonSetName, hostID, templateHash string, slot int) string {
	suffix := fmt.Sprintf("-%s-%s-%d", shortHash(hostID), templateHash, slot)
	const maxNameLength = 253
	if len(daemonSetName)+len(suffix) > maxNameLength {
		daemonSetName = strings.TrimRight(daemonSetName[:maxNameLength-len(suffix)], ".-")
	}
	return daemonSetName + suffix
}

// matchingHosts returns the schedulable Hosts that spec selects, keyed by
// HostID with their last Ready transition, and the matching Hosts that are
// not schedulable right now but still exist. Instances on a retained Host
// are left alone.
func (r *WorkloadDaemonSetReconciler) matchingHosts(ctx context.Context, namespace string, spec *runtimev1alpha1.WorkloadSpec) (map[string]time.Time, map[string]bool, error) {
	environmentFilter, err := hostEnvironmentFilter(r.AllowSharedHosts, namespace, spec)
	if err != nil {
		return nil, nil, err
	}

	listOpts := []client.ListOption{
		client.InNamespace(r.OperatorNamespace),
		client.MatchingLabels(spec.HostSelector),
	}
	if environmentFilter != "" {
		listOpts = append(listOpts, client.MatchingFields{hostEnvironmentIndex: environmentFilter})
	}

	hostList := &runtimev1alpha1.HostList{}
	if err := r.List(ctx, hostList, listOpts...); err != nil {
		return nil, nil, err
	}

	desired := make(map[string]time.Time)
	retained := make(map[string]bool)
	for i := range hostList.Items {
		host := &hostList.Items[i]
		switch {
		case host.HostID == "":
		case isSchedulable(host):
			desired[host.HostID] = host.Status.GetCondition(condition.TypeReady).LastTransitionTime.Time
		case host.DeletionTimestamp.IsZero():
			retained[host.HostID] = true
		}
	}
	return desired, retained, nil
}

func (r *WorkloadDaemonSetReconciler) setScheduleStatus(
	ctx context.Context,
	daemonSet *runtimev1alpha1.WorkloadDaemonSet,
	desired []string,
	hosts map[string]*daemonSetHost,
	perHost int32,
) {
	condition.ForceStatusUpdate(ctx)

	status := &daemonSet.Status
	status.DesiredHosts = int32(len(desired))
	status.ReadyHosts = 0
	status.UpdatedHosts = 0
	status.Hosts = nil

	replicas := &runtimev1alpha1.ReplicaSetStatus{Expected: status.DesiredHosts * perHost}
	for _, hostID := range desired {
		host := hosts[hostID]
		hostStatus := runtimev1alpha1.WorkloadDaemonSetHostStatus{
			HostID:   hostID,
			Current:  int32(len(host.current)),
			Ready:    host.readyCurrent(),
			Outdated: int32(len(host.outdated)),
		}
		ready, updated := hostStatus.Ready >= perHost, hostStatus.Outdated == 0
		if ready {
			status.ReadyHosts++
		}
		if updated {
			status.UpdatedHosts++
		}
		if !ready || !updated {
			status.Hosts = append(status.Hosts, hostStatus)
		}
		replicas.Current += hostStatus.Current
		replicas.Ready += hostStatus.Ready
		replicas.Unavailable += hostStatus.Current - hostStatus.Ready
	}
	status.Replicas = replicas
}

func (r *WorkloadDaemonSetReconciler) createWorkload(
	ctx context.Context,
	daemonSet *runtimev1alpha1.WorkloadDaemonSet,
	template *runtimev1alpha1.WorkloadReplicaTemplate,
	templateHash string,
	hostID string,
	name string,
) (*runtimev1alpha1.Workload, error) {
	annotations := make(map[string]string, len(template.Annotations)+2)
	for k, v := range template.Annotations {
		// The host names its workload from these, so a copied one would
		// report this DaemonSet's metrics under another owner.
		if k == workloadReplicaSetGenerationAnnotation ||
			k == workloadReplicaSetNameAnnotation ||
			k == workloadDeploymentNameAnnotation ||
			k == workloadDaemonSetGenerationAnnotation ||
			k == workloadDaemonSetNameAnnotation {
			continue
		}
		annotations[k] = v
	}
	annotations[workloadDaemonSetGenerationAnnotation] = templateHash
	annotations[workloadDaemonSetNameAnnotation] = daemonSet.Name

	workloadLabels := make(map[string]string, len(template.Labels)+1)
	maps.Copy(workloadLabels, template.Labels)
	workloadLabels[workloadDaemonSetNameLabel] = ownerLabelValue(daemonSet.Name)

	spec := template.Spec.DeepCopy()
	spec.HostID = hostID

	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{
			Name:        name,
			Namespace:   daemonSet.Namespace,
			Labels:      workloadLabels,
			Annotations: annotations,
		},
		Spec: *spec,
	}
	if err := controllerutil.SetControllerReference(daemonSet, workload, r.Scheme); err != nil {
		return nil, err
	}
	err := r.Create(ctx, workload)
	if !apierrors.IsAlreadyExists(err) {
		return workload, err
	}
	return r.existingInstance(ctx, daemonSet, name)
}

// existingInstance resolves a create that hit an existing name. It reads past
// the cache, which may not hold this DaemonSet's own create yet. Anything this
// DaemonSet doesn't control is holding the slot, which is an error.
func (r *WorkloadDaemonSetReconciler) existingInstance(
	ctx context.Context,
	daemonSet *runtimev1alpha1.WorkloadDaemonSet,
	name string,
) (*runtimev1alpha1.Workload, error) {
	existing := &runtimev1alpha1.Workload{}
	if err := r.APIReader.Get(ctx, client.ObjectKey{Namespace: daemonSet.Namespace, Name: name}, existing); err != nil {
		return nil, err
	}
	if owner := metav1.GetControllerOf(existing); owner == nil || owner.UID != daemonSet.UID {
		return nil, fmt.Errorf("workload %s already exists and this WorkloadDaemonSet does not control it", name)
	}
	return existing, nil
}

// adoptOrphans takes over the Workloads that carry this DaemonSet's label but
// have no controller, such as those --cascade=orphan left behind, so they roll,
// scale and leave like any other instance. It returns them for this pass.
func (r *WorkloadDaemonSetReconciler) adoptOrphans(
	ctx context.Context,
	daemonSet *runtimev1alpha1.WorkloadDaemonSet,
) ([]runtimev1alpha1.Workload, error) {
	labelled := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, labelled,
		client.InNamespace(daemonSet.Namespace),
		client.MatchingLabels{workloadDaemonSetNameLabel: ownerLabelValue(daemonSet.Name)},
	); err != nil {
		return nil, err
	}
	orphans := labelled.Items[:0]
	for i := range labelled.Items {
		if metav1.GetControllerOf(&labelled.Items[i]) == nil {
			orphans = append(orphans, labelled.Items[i])
		}
	}
	if len(orphans) == 0 {
		return nil, nil
	}

	// Deleting a DaemonSet is what orphans its Workloads, and the cached copy
	// may not show that delete yet.
	latest := &runtimev1alpha1.WorkloadDaemonSet{}
	if err := r.APIReader.Get(ctx, client.ObjectKeyFromObject(daemonSet), latest); client.IgnoreNotFound(err) != nil {
		return nil, err
	}
	if latest.UID != daemonSet.UID || !latest.DeletionTimestamp.IsZero() {
		return nil, condition.ErrNoop()
	}

	adopted := orphans[:0]
	for i := range orphans {
		workload := &orphans[i]
		if workload.DeletionTimestamp.IsZero() {
			base := workload.DeepCopy()
			if err := controllerutil.SetControllerReference(daemonSet, workload, r.Scheme); err != nil {
				return nil, err
			}
			if err := r.Patch(ctx, workload, client.MergeFrom(base)); err != nil {
				if apierrors.IsNotFound(err) {
					continue
				}
				return nil, err
			}
		}
		adopted = append(adopted, *workload)
	}
	return adopted, nil
}

func (r *WorkloadDaemonSetReconciler) deleteAll(ctx context.Context, workloads []*runtimev1alpha1.Workload) error {
	for _, workload := range workloads {
		if err := r.Delete(ctx, workload); client.IgnoreNotFound(err) != nil {
			return err
		}
	}
	return nil
}

func (r *WorkloadDaemonSetReconciler) reconcileReady(_ context.Context, daemonSet *runtimev1alpha1.WorkloadDaemonSet) error {
	if !daemonSet.Status.AllTrue(
		runtimev1alpha1.WorkloadDaemonSetConditionArtifact,
		runtimev1alpha1.WorkloadDaemonSetConditionSchedule) {
		return condition.ErrStatusUnknown(fmt.Errorf("daemonset is not scheduled"))
	}
	status := daemonSet.Status
	if status.ReadyHosts != status.DesiredHosts || status.UpdatedHosts != status.DesiredHosts {
		return condition.ErrStatusUnknown(fmt.Errorf(
			"hosts not ready: desired=%d ready=%d updated=%d",
			status.DesiredHosts, status.ReadyHosts, status.UpdatedHosts))
	}
	return nil
}

// daemonSetsForHost enqueues the DaemonSets that select host. It runs on
// both the old and new Host of an update, which covers a Host that stops
// matching as well as one that starts.
func (r *WorkloadDaemonSetReconciler) daemonSetsForHost(ctx context.Context, obj client.Object) []reconcile.Request {
	host, ok := obj.(*runtimev1alpha1.Host)
	if !ok {
		return nil
	}
	daemonSets := &runtimev1alpha1.WorkloadDaemonSetList{}
	if err := r.List(ctx, daemonSets); err != nil {
		ctrl.LoggerFrom(ctx).Error(err, "listing WorkloadDaemonSets for Host event")
		return nil
	}
	var requests []reconcile.Request
	for _, ds := range daemonSets.Items {
		spec := &ds.Spec.Template.Spec
		if !labels.SelectorFromSet(spec.HostSelector).Matches(labels.Set(host.Labels)) {
			continue
		}
		filter, err := hostEnvironmentFilter(r.AllowSharedHosts, ds.Namespace, spec)
		if err != nil || (filter != "" && filter != host.Environment) {
			continue
		}
		requests = append(requests, reconcile.Request{
			NamespacedName: types.NamespacedName{Namespace: ds.Namespace, Name: ds.Name},
		})
	}
	return requests
}

// hostPlacementChanged passes Host updates that can change which DaemonSets
// place on it, skipping the heartbeat status writes that change nothing.
var hostPlacementChanged = predicate.Funcs{
	UpdateFunc: func(e event.UpdateEvent) bool {
		oldHost, okOld := e.ObjectOld.(*runtimev1alpha1.Host)
		newHost, okNew := e.ObjectNew.(*runtimev1alpha1.Host)
		if !okOld || !okNew {
			return true
		}
		return isSchedulable(oldHost) != isSchedulable(newHost) ||
			oldHost.HostID != newHost.HostID ||
			oldHost.Environment != newHost.Environment ||
			!maps.Equal(oldHost.Labels, newHost.Labels)
	},
}

// SetupWithManager sets up the controller with the Manager.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloaddaemonsets,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloaddaemonsets/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloaddaemonsets/finalizers,verbs=update

func (r *WorkloadDaemonSetReconciler) SetupWithManager(mgr ctrl.Manager) error {
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.WorkloadDaemonSet{},
		workloadDaemonSetReconcileInterval)

	reconciler.SetCondition(runtimev1alpha1.WorkloadDaemonSetConditionArtifact, r.reconcileArtifacts)
	reconciler.SetCondition(runtimev1alpha1.WorkloadDaemonSetConditionSchedule, r.reconcileSchedule)
	reconciler.SetCondition(condition.TypeReady, r.reconcileReady)

	r.reconciler = reconciler

	daemonSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadDaemonSet{}, r.Scheme)
	if err != nil {
		return err
	}

	err = mgr.GetFieldIndexer().IndexField(context.Background(), &runtimev1alpha1.Workload{}, workloadDaemonSetNameIndex, func(rawObj client.Object) []string {
		if ownerName, ok := isOwnedByController(rawObj, daemonSetGvk); ok {
			return []string{ownerName}
		}

		return []string{}
	})
	if err != nil {
		return err
	}

	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.WorkloadDaemonSet{}).
		Owns(&runtimev1alpha1.Workload{}).
		Watches(&runtimev1alpha1.Host{},
			handler.EnqueueRequestsFromMapFunc(r.daemonSetsForHost),
			builder.WithPredicates(hostPlacementChanged)).
		Named("workload-daemonset").
		Complete(r)
}
