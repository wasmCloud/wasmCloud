package runtime

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

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
	for slot, added := 0, 0; added < missing; slot++ {
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
				Name:        name,
				Namespace:   replicaSet.Namespace,
				Labels:      workloadLabels,
				Annotations: annotations,
			},
			Spec: replicaSet.Spec.Template.Spec,
		}

		if err := controllerutil.SetControllerReference(replicaSet, workload, r.Scheme); err != nil {
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
					!apiequality.Semantic.DeepDerivative(workload.Spec, existing.Spec) {
					return fmt.Errorf("workload %s exists with different ownership or spec, or is terminating", name)
				}
				continue
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

	for i := len(workloads.Items); i > expectedReplicas; i-- {
		if err := r.Delete(ctx, &workloads.Items[i-1]); err != nil {
			return err
		}
	}

	return nil
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
		if workload.Status.IsAvailable() {
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

	for _, workload := range workloads.Items {
		// catches workloads already being deleted
		if workload.DeletionTimestamp != nil {
			continue
		}

		// catches workloads that are of the current generation
		if generation, ok := workload.Annotations[workloadReplicaSetGenerationAnnotation]; !ok || generation == currentGeneration {
			continue
		}

		// At this point, the workload is not of the current generation
		// and should be cleaned up
		if err := r.Delete(ctx, &workload); err != nil {
			return err
		}
	}
	return nil
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

	for _, workload := range workloads.Items {
		if !isFailedPastGracePeriod(&workload, time.Time{}) {
			continue
		}
		// Delete the workload, which will be replaced by the scale-up logic
		if err := r.Delete(ctx, &workload); err != nil {
			return err
		}
	}
	return nil
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
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.WorkloadReplicaSet{},
		workloadReplicaSetReconcileInterval)
	reconciler.ReadyRequeueInterval(r.ReadyRequeueInterval)
	reconciler.OptimisticStatusWrites()

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
		Owns(&runtimev1alpha1.Workload{}).
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
