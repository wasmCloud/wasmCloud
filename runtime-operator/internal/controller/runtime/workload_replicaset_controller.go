package runtime

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"

	"golang.org/x/sync/errgroup"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/handler"

	apiequality "k8s.io/apimachinery/pkg/api/equality"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/labels"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/client-go/tools/events"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	workloadReplicaSetReconcileInterval    = 1 * time.Minute
	workloadReplicaSetReplicaGracePeriod   = 1 * time.Minute
	workloadReplicaSetOwnerIndex           = "workload.replicaset.owner"
	workloadReplicaSetRevisionIndex        = "workload.replicaset.revision"
	workloadReplicaSetGenerationAnnotation = "runtime.wasmcloud.dev/workload-replica-set-generation"
	workloadReplicaSetNameAnnotation       = "runtime.wasmcloud.dev/workload-replicaset-name"
	workloadDeploymentNameAnnotation       = "runtime.wasmcloud.dev/workload-deployment"
	// workloadReplicaSetNameLabel is stamped onto the Workloads a ReplicaSet
	// owns and exposed via its /scale subresource's selector. Its value is
	// ownerLabelValue(name), since an RS name can exceed 63 characters.
	workloadReplicaSetNameLabel = "runtime.wasmcloud.dev/workload-replicaset"
	// workloadReplicaSetUIDLabel narrows that selector to this ReplicaSet, since
	// a deleted one's Workloads keep the same name label until they are gone.
	workloadReplicaSetUIDLabel = "runtime.wasmcloud.dev/workload-replicaset-uid"
)

// WorkloadReplicaSetReconciler reconciles a WorkloadReplicaSet object
type WorkloadReplicaSetReconciler struct {
	client.Client
	ReadyRequeueInterval time.Duration
	BatchSize            int
	Concurrency          int
	APIReader            client.Reader
	Scheme               *runtime.Scheme
	Recorder             events.EventRecorder
	reconciler           condition.AnyConditionedReconciler
}

// recordRefusedCreate records a Workload create that the API server refused
// on the ReplicaSet and on the WorkloadDeployment that owns it, where the
// user is looking.
func (r *WorkloadReplicaSetReconciler) recordRefusedCreate(replicaSet *runtimev1alpha1.WorkloadReplicaSet, err error) {
	recordRefused(r.Recorder, replicaSet, nil, "Create", "Workload create refused", err)
	if owner := metav1.GetControllerOf(replicaSet); owner != nil &&
		owner.APIVersion == runtimev1alpha1.GroupVersion.String() && owner.Kind == "WorkloadDeployment" {
		deployment := &runtimev1alpha1.WorkloadDeployment{ObjectMeta: metav1.ObjectMeta{
			Name:      owner.Name,
			Namespace: replicaSet.Namespace,
			UID:       owner.UID,
		}}
		recordRefused(r.Recorder, deployment, replicaSet, "Create",
			fmt.Sprintf("WorkloadReplicaSet %s: Workload create refused", replicaSet.Name), err)
	}
}

func (r *WorkloadReplicaSetReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	return r.reconciler.Reconcile(ctx, req)
}

func (r *WorkloadReplicaSetReconciler) reconcileScaleUp(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
	// The garbage collector is removing or orphaning this ReplicaSet's Workloads.
	if !replicaSet.DeletionTimestamp.IsZero() {
		return condition.ErrNoop()
	}

	if replicaSet.Spec.Replicas == nil {
		return nil
	}

	templateHash := replicaSet.Spec.Template.Hash()
	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(
		ctx,
		workloads,
		client.InNamespace(replicaSet.Namespace),
		client.MatchingFields{
			workloadReplicaSetRevisionIndex: replicaSetRevisionKey(string(replicaSet.UID), templateHash),
		},
	); err != nil {
		return err
	}

	expectedReplicas := int(*replicaSet.Spec.Replicas)

	names := make(map[string]bool, len(workloads.Items))
	for _, workload := range workloads.Items {
		names[workload.Name] = true
	}
	missing := expectedReplicas - len(workloads.Items)
	if missing <= 0 {
		return nil
	}
	if err := r.confirmScale(ctx, replicaSet); err != nil {
		return err
	}
	count := min(missing, r.batchSize())
	if missing > count {
		condition.ForceRequeue(ctx)
	}
	createCtx, cancelCreates := context.WithCancel(ctx)
	defer cancelCreates()
	creates, createCtx := errgroup.WithContext(createCtx)
	creates.SetLimit(max(1, r.Concurrency))
	for slot, added := 0, 0; added < count; slot++ {
		name := rolloutChildName(replicaSet.Name, fmt.Sprintf("%s:%s:%d",
			replicaSet.UID, templateHash, slot))
		if names[name] {
			continue
		}
		added++
		annotations := make(map[string]string, len(replicaSet.Spec.Template.Annotations)+3)
		for k, v := range replicaSet.Spec.Template.Annotations {
			if k == workloadReplicaSetGenerationAnnotation ||
				k == workloadReplicaSetNameAnnotation ||
				k == workloadDeploymentNameAnnotation {
				continue
			}
			annotations[k] = v
		}
		annotations[workloadReplicaSetGenerationAnnotation] = templateHash
		annotations[workloadReplicaSetNameAnnotation] = replicaSet.Name
		if replicaSet.Labels != nil && replicaSet.Labels[workloadDeploymentNameLabel] != "" {
			annotations[workloadDeploymentNameAnnotation] = replicaSet.Labels[workloadDeploymentNameLabel]
		}

		workloadLabels := make(map[string]string, len(replicaSet.Spec.Template.Labels)+2)
		for k, v := range replicaSet.Spec.Template.Labels {
			workloadLabels[k] = v
		}
		workloadLabels[workloadReplicaSetNameLabel] = ownerLabelValue(replicaSet.Name)
		workloadLabels[workloadReplicaSetUIDLabel] = string(replicaSet.UID)

		workload := &runtimev1alpha1.Workload{
			ObjectMeta: metav1.ObjectMeta{
				Finalizers:  []string{workloadFinalizerName},
				Name:        name,
				Namespace:   replicaSet.Namespace,
				Labels:      workloadLabels,
				Annotations: annotations,
			},
			Spec: replicaSet.Spec.Template.Spec,
		}

		if err := controllerutil.SetControllerReference(replicaSet, workload, r.Scheme); err != nil {
			cancelCreates()
			return errors.Join(err, creates.Wait())
		}

		creates.Go(func() error { return r.createReplica(createCtx, replicaSet, workload, templateHash) })
	}
	return creates.Wait()
}

func (r *WorkloadReplicaSetReconciler) createReplica(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet, workload *runtimev1alpha1.Workload, templateHash string) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := r.Create(ctx, workload); err != nil {
		if apierrors.IsAlreadyExists(err) {
			reader := r.APIReader
			if reader == nil {
				reader = r.Client
			}
			existing := &runtimev1alpha1.Workload{}
			if err := reader.Get(ctx, client.ObjectKeyFromObject(workload), existing); err != nil {
				return err
			}
			if !existing.DeletionTimestamp.IsZero() || !metav1.IsControlledBy(existing, replicaSet) ||
				existing.Annotations[workloadReplicaSetGenerationAnnotation] != templateHash ||
				!apiequality.Semantic.DeepDerivative(workload.Spec, existing.Spec) {
				return fmt.Errorf("workload %s exists with different ownership, revision, or spec, or is terminating", workload.Name)
			}
			return nil
		}
		// A refusal may mention the Workload
		// name: the API server quotes it, a webhook may use it anywhere.
		// Left in, the ScaleUp message would change every pass, and a
		// changed condition is requeued a second later: the create, and its
		// events, would repeat each second.
		refused := isRefused(err)
		msg := strings.ReplaceAll(err.Error(), fmt.Sprintf(" %q", workload.Name), "")
		err = errors.New(strings.ReplaceAll(msg, workload.Name, "<new Workload>"))
		if refused {
			r.recordRefusedCreate(replicaSet, err)
		}
		return fmt.Errorf("creating Workload: %w", err)
	}
	return nil
}

func (r *WorkloadReplicaSetReconciler) reconcileScaleDown(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
	if !replicaSet.DeletionTimestamp.IsZero() {
		return condition.ErrNoop()
	}

	expectedReplicas := 0
	if replicaSet.Spec.Replicas != nil {
		expectedReplicas = int(*replicaSet.Spec.Replicas)
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(
		ctx,
		workloads,
		client.InNamespace(replicaSet.Namespace),
		client.MatchingFields{
			workloadReplicaSetRevisionIndex: replicaSetRevisionKey(string(replicaSet.UID), replicaSet.Spec.Template.Hash()),
		},
	); err != nil {
		return err
	}

	// Terminating replicas already satisfy earlier deletion decisions. Retain
	// the same lexicographic prefix on every pass, independent of list order.
	live := make([]runtimev1alpha1.Workload, 0, len(workloads.Items))
	for _, w := range workloads.Items {
		if w.DeletionTimestamp.IsZero() {
			live = append(live, w)
		}
	}
	slices.SortFunc(live, func(a, b runtimev1alpha1.Workload) int { return cmp.Compare(a.Name, b.Name) })
	excess := len(live) - expectedReplicas
	if excess <= 0 {
		return nil
	}
	if err := r.confirmScale(ctx, replicaSet); err != nil {
		return err
	}
	count := min(excess, r.batchSize())
	if excess > count {
		condition.ForceRequeue(ctx)
	}
	deletes, deleteCtx := errgroup.WithContext(ctx)
	deletes.SetLimit(max(1, r.Concurrency))
	for i := len(live) - 1; i >= len(live)-count; i-- {
		workload := &live[i]
		deletes.Go(func() error {
			if err := deleteCtx.Err(); err != nil {
				return err
			}
			return deleteObservedObject(deleteCtx, r.Client, workload)
		})
	}
	return deletes.Wait()
}

func (r *WorkloadReplicaSetReconciler) reconcileReady(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
	deleting := !replicaSet.DeletionTimestamp.IsZero()
	if !deleting {
		if failed, ok := firstFailed(&replicaSet.Status.ConditionedStatus,
			runtimev1alpha1.WorkloadReplicaSetConditionScaleUp, runtimev1alpha1.WorkloadReplicaSetConditionScaleDown); ok {
			return fmt.Errorf("%s: %s", failed.Type, failed.Message)
		}
	}
	if !deleting && !replicaSet.Status.AllTrue(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp, runtimev1alpha1.WorkloadReplicaSetConditionScaleDown) {
		return fmt.Errorf("scaling in progress")
	}

	expectedReplicas := int32(0)
	if replicaSet.Spec.Replicas != nil {
		expectedReplicas = *replicaSet.Spec.Replicas
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(
		ctx,
		workloads,
		client.InNamespace(replicaSet.Namespace),
		client.MatchingFields{
			workloadReplicaSetRevisionIndex: replicaSetRevisionKey(string(replicaSet.UID), replicaSet.Spec.Template.Hash()),
		},
	); err != nil {
		return err
	}
	currentReplicas := int32(len(workloads.Items))

	unavailableCount := int32(0)
	readyCount := int32(0)
	for _, workload := range workloads.Items {
		if workload.DeletionTimestamp.IsZero() && workload.Status.IsAvailable() {
			readyCount++
		} else {
			unavailableCount++
		}
	}

	condition.ForceStatusUpdate(ctx)

	replicaStatus := &runtimev1alpha1.ReplicaSetStatus{
		Expected:    expectedReplicas,
		Current:     currentReplicas,
		Ready:       readyCount,
		Unavailable: unavailableCount,
	}

	replicaSet.Status.Replicas = replicaStatus
	replicaSet.Status.CurrentReplicas = currentReplicas
	replicaSet.Status.Selector = labels.Set{
		workloadReplicaSetNameLabel: ownerLabelValue(replicaSet.Name),
		workloadReplicaSetUIDLabel:  string(replicaSet.UID),
	}.String()

	if deleting {
		return condition.ErrStatusUnknown(fmt.Errorf("replicaset is being deleted"))
	}

	if unavailableCount > 0 {
		return fmt.Errorf("%d workloads are not available", unavailableCount)
	}

	if currentReplicas != expectedReplicas {
		return fmt.Errorf("expected %d workloads, got %d", expectedReplicas, currentReplicas)
	}

	return nil
}

func (r *WorkloadReplicaSetReconciler) reconcileCleanup(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
	if !replicaSet.DeletionTimestamp.IsZero() {
		return nil
	}

	if !replicaSet.Status.AllTrue(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp, runtimev1alpha1.WorkloadReplicaSetConditionScaleDown) {
		// wait until scaling is done
		return nil
	}

	if err := r.cleanupPreviousRevisionsWorkloads(ctx, replicaSet); err != nil {
		return err
	}

	return r.cleanupUnhealthyWorkloads(ctx, replicaSet)
}

func (r *WorkloadReplicaSetReconciler) cleanupPreviousRevisionsWorkloads(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(
		ctx,
		workloads,
		client.InNamespace(replicaSet.Namespace),
		client.MatchingFields{
			workloadReplicaSetOwnerIndex: string(replicaSet.UID),
		},
	); err != nil {
		return err
	}

	currentGeneration := replicaSet.Spec.Template.Hash()
	victims := make([]runtimev1alpha1.Workload, 0)
	for _, workload := range workloads.Items {
		if workload.DeletionTimestamp != nil {
			continue
		}
		if generation, ok := workload.Annotations[workloadReplicaSetGenerationAnnotation]; ok && generation != currentGeneration {
			victims = append(victims, workload)
		}
	}
	return r.deleteReplicaBatch(ctx, replicaSet, victims)
}

func (r *WorkloadReplicaSetReconciler) cleanupUnhealthyWorkloads(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(
		ctx,
		workloads,
		client.InNamespace(replicaSet.Namespace),
		client.MatchingFields{
			workloadReplicaSetRevisionIndex: replicaSetRevisionKey(string(replicaSet.UID), replicaSet.Spec.Template.Hash()),
		},
	); err != nil {
		return err
	}

	victims := make([]runtimev1alpha1.Workload, 0)
	for _, workload := range workloads.Items {
		if isFailedPastGracePeriod(&workload, time.Time{}) {
			victims = append(victims, workload)
		}
	}
	return r.deleteReplicaBatch(ctx, replicaSet, victims)
}

func (r *WorkloadReplicaSetReconciler) deleteReplicaBatch(ctx context.Context, rs *runtimev1alpha1.WorkloadReplicaSet, victims []runtimev1alpha1.Workload) error {
	if len(victims) == 0 {
		return nil
	}
	if err := r.confirmScale(ctx, rs); err != nil {
		return err
	}
	slices.SortFunc(victims, func(a, b runtimev1alpha1.Workload) int { return cmp.Compare(a.Name, b.Name) })
	count := min(len(victims), r.batchSize())
	if len(victims) > count {
		condition.ForceRequeue(ctx)
	}
	deletes, deleteCtx := errgroup.WithContext(ctx)
	deletes.SetLimit(max(1, r.Concurrency))
	for i := range count {
		workload := &victims[i]
		deletes.Go(func() error {
			if err := deleteCtx.Err(); err != nil {
				return err
			}
			return deleteObservedObject(deleteCtx, r.Client, workload)
		})
	}
	return deletes.Wait()
}

// isFailedPastGracePeriod reports whether workload is not ready, has failed,
// and has not recovered within the grace period, so it should be replaced.
// The grace period starts at the Sync transition or notBefore, whichever is later.
func isFailedPastGracePeriod(workload *runtimev1alpha1.Workload, notBefore time.Time) bool {
	// catches workloads already being deleted
	if workload.DeletionTimestamp != nil {
		return false
	}
	// catches ready workloads
	if workload.Status.IsAvailable() {
		return false
	}

	// catches workloads that are still syncing
	// they might have not been placed or sync failing. in most cases host is down/gone.

	syncStatus := workload.Status.GetCondition(runtimev1alpha1.WorkloadConditionSync)
	if syncStatus.Status == condition.ConditionTrue {
		return false
	}

	// this was the first failure, give it some time to recover
	if syncStatus.LastTransitionTime.IsZero() {
		return false
	}

	// If the workload has failed recently, give it some time to recover
	failedAt := syncStatus.LastTransitionTime.Time
	if notBefore.After(failedAt) {
		failedAt = notBefore
	}
	return !failedAt.Add(workloadReplicaSetReplicaGracePeriod).After(time.Now())
}

// SetupWithManager sets up the controller with the Manager.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloadreplicasets,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloadreplicasets/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloadreplicasets/finalizers,verbs=update

func (r *WorkloadReplicaSetReconciler) SetupWithManager(mgr ctrl.Manager) error {
	r.APIReader = mgr.GetAPIReader()
	if r.BatchSize <= 0 {
		r.BatchSize = DefaultReplicaBatchSize
	}
	if r.Concurrency <= 0 {
		r.Concurrency = DefaultReplicaConcurrency
	}
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.WorkloadReplicaSet{},
		workloadReplicaSetReconcileInterval)
	reconciler.ReadyRequeueInterval(r.ReadyRequeueInterval)
	reconciler.OptimisticStatusWrites()
	reconciler.BatchStatusUpdates()

	// WorkloadDeployment reads these conditions and must tell a current one
	// from one an earlier spec left behind.
	reconciler.TrackGeneration()
	reconciler.SetCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp, r.reconcileScaleUp)
	reconciler.SetCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleDown, r.reconcileScaleDown)
	reconciler.SetCondition(condition.TypeReady, r.reconcileReady)
	reconciler.AddPostHook(r.reconcileCleanup)

	r.reconciler = reconciler

	// NOTE(lxf): We only touch Replicas that have been setup via ReplicaSet
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, r.Scheme)
	if err != nil {
		return err
	}

	err = mgr.GetFieldIndexer().IndexField(context.Background(), &runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, replicaSetOwnerIndexValue(replicaSetGvk))
	if err != nil {
		return err
	}

	err = mgr.GetFieldIndexer().IndexField(context.Background(), &runtimev1alpha1.Workload{}, workloadReplicaSetRevisionIndex, replicaSetRevisionIndexValue(replicaSetGvk))
	if err != nil {
		return err
	}

	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.WorkloadReplicaSet{}).
		Watches(&runtimev1alpha1.Workload{}, coalesce(handler.EnqueueRequestForOwner(mgr.GetScheme(), mgr.GetRESTMapper(), &runtimev1alpha1.WorkloadReplicaSet{}, handler.OnlyControllerOwner())), builder.WithPredicates(childChanges())).
		Named("workload-replicaset").
		Complete(r)
}

// replicaSetOwnerIndexValue indexes a Workload by its controlling ReplicaSet UID.
func replicaSetOwnerIndexValue(replicaSetGvk schema.GroupVersionKind) client.IndexerFunc {
	return func(rawObj client.Object) []string {
		if ownerUID, ok := controllerUID(rawObj, replicaSetGvk); ok {
			return []string{ownerUID}
		}

		return []string{}
	}
}

func replicaSetRevisionKey(ownerUID, templateHash string) string {
	return ownerUID + ":" + templateHash
}

// replicaSetRevisionIndexValue combines the owner UID and template hash.
// A cache query with both separate indexes can select the hash first and
// scan every Workload that uses the template, even for a single replica.
func replicaSetRevisionIndexValue(replicaSetGvk schema.GroupVersionKind) client.IndexerFunc {
	return func(rawObj client.Object) []string {
		ownerUID, owned := controllerUID(rawObj, replicaSetGvk)
		if generation, stamped := rawObj.GetAnnotations()[workloadReplicaSetGenerationAnnotation]; owned && stamped {
			return []string{replicaSetRevisionKey(ownerUID, generation)}
		}
		return nil
	}
}

func (r *WorkloadReplicaSetReconciler) batchSize() int {
	if r.BatchSize > 0 {
		return r.BatchSize
	}
	return DefaultReplicaBatchSize
}

// A fresh parent read bounds the damage from a scale decision made against
// stale cached intent. Child deletes still carry UID and resourceVersion guards.
func (r *WorkloadReplicaSetReconciler) confirmScale(ctx context.Context, observed *runtimev1alpha1.WorkloadReplicaSet) error {
	if r.APIReader == nil {
		return nil
	}
	current := &runtimev1alpha1.WorkloadReplicaSet{}
	if err := r.APIReader.Get(ctx, client.ObjectKeyFromObject(observed), current); err != nil {
		return err
	}
	if current.UID != observed.UID || current.Generation != observed.Generation ||
		!current.DeletionTimestamp.IsZero() || !apiequality.Semantic.DeepEqual(current.Spec, observed.Spec) {
		condition.ForceRequeue(ctx)
		return condition.ErrStatusUnknown(fmt.Errorf("replicaset changed while planning scale; retrying"))
	}
	return nil
}
