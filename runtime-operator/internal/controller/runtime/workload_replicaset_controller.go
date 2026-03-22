package runtime

import (
	"context"
	"fmt"
	"time"

	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	"go.wasmcloud.dev/runtime-operator/api/condition"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/api/runtime/v1alpha1"
)

const (
	workloadReplicaSetReconcileInterval    = 1 * time.Minute
	workloadReplicaSetReplicaGracePeriod   = 1 * time.Minute
	workloadReplicaSetNameIndex            = "workload.replicaset.name"
	workloadReplicaSetHashIndex            = "workload.replicaset.hash"
	workloadReplicaSetGenerationAnnotation = "runtime.wasmcloud.dev/workload-replica-set-generation"
)

// WorkloadReplicaSetReconciler reconciles a WorkloadReplicaSet object
type WorkloadReplicaSetReconciler struct {
	client.Client
	Scheme     *runtime.Scheme
	reconciler condition.AnyConditionedReconciler
}

func (r *WorkloadReplicaSetReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	return r.reconciler.Reconcile(ctx, req)
}

func (r *WorkloadReplicaSetReconciler) reconcileScaleUp(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
	if replicaSet.Spec.Replicas == nil {
		return nil
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(
		ctx,
		workloads,
		client.InNamespace(replicaSet.Namespace),
		client.MatchingFields{
			workloadReplicaSetNameIndex: replicaSet.Name,
			workloadReplicaSetHashIndex: replicaSet.Spec.Template.Hash(),
		},
	); err != nil {
		return err
	}

	expectedReplicas := int(*replicaSet.Spec.Replicas)

	for i := len(workloads.Items); i < expectedReplicas; i++ {
		workload := &runtimev1alpha1.Workload{
			ObjectMeta: metav1.ObjectMeta{
				Name:      fmt.Sprintf("%s-%s", replicaSet.Name, randHash()),
				Namespace: replicaSet.Namespace,
				Labels:    replicaSet.Spec.Template.Labels,
				Annotations: map[string]string{
					workloadReplicaSetGenerationAnnotation: replicaSet.Spec.Template.Hash(),
				},
			},
			Spec: replicaSet.Spec.Template.Spec,
		}

		if err := controllerutil.SetControllerReference(replicaSet, workload, r.Scheme); err != nil {
			return err
		}

		if err := r.Create(ctx, workload); err != nil {
			return err
		}
	}

	return nil
}

func (r *WorkloadReplicaSetReconciler) reconcileScaleDown(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
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
			workloadReplicaSetNameIndex: replicaSet.Name,
			workloadReplicaSetHashIndex: replicaSet.Spec.Template.Hash(),
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
	if !replicaSet.Status.AllTrue(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp, runtimev1alpha1.WorkloadReplicaSetConditionScaleDown) {
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
			workloadReplicaSetNameIndex: replicaSet.Name,
			workloadReplicaSetHashIndex: replicaSet.Spec.Template.Hash(),
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

	if unavailableCount > 0 {
		return fmt.Errorf("%d workloads are not available", unavailableCount)
	}

	if currentReplicas != expectedReplicas {
		return fmt.Errorf("expected %d workloads, got %d", expectedReplicas, currentReplicas)
	}

	return nil
}

func (r *WorkloadReplicaSetReconciler) reconcileCleanup(ctx context.Context, replicaSet *runtimev1alpha1.WorkloadReplicaSet) error {
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
			workloadReplicaSetNameIndex: replicaSet.Name,
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
			workloadReplicaSetNameIndex: replicaSet.Name,
			workloadReplicaSetHashIndex: replicaSet.Spec.Template.Hash(),
		},
	); err != nil {
		return err
	}

	for _, workload := range workloads.Items {
		// catches workloads already being deleted
		if workload.DeletionTimestamp != nil {
			continue
		}
		// catches ready workloads
		if workload.Status.IsAvailable() {
			continue
		}

		// catches workloads that are still syncing
		// they might have not been placed or sync failing. in most cases host is down/gone.

		syncStatus := workload.Status.GetCondition(runtimev1alpha1.WorkloadConditionSync)
		if syncStatus.Status == condition.ConditionTrue {
			continue
		}

		// this was the first failure, give it some time to recover
		if syncStatus.LastTransitionTime.IsZero() {
			continue
		}

		// If the workload has failed recently, give it some time to recover
		if syncStatus.LastTransitionTime.Time.Add(workloadReplicaSetReplicaGracePeriod).After(time.Now()) {
			continue
		}
		// At this point, the workload is not ready, has failed, and has not recovered within the grace period
		// Delete the workload, which will be replaced by the scale-up logic
		if err := r.Delete(ctx, &workload); err != nil {
			return err
		}
	}
	return nil
}

// SetupWithManager sets up the controller with the Manager.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloadreplicasets,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloadreplicasets/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=workloadreplicasets/finalizers,verbs=update

func (r *WorkloadReplicaSetReconciler) SetupWithManager(mgr ctrl.Manager) error {
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.WorkloadReplicaSet{},
		workloadReplicaSetReconcileInterval)

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

	err = mgr.GetFieldIndexer().IndexField(context.Background(), &runtimev1alpha1.Workload{}, workloadReplicaSetNameIndex, func(rawObj client.Object) []string {
		if ownerName, ok := isOwnedByController(rawObj, replicaSetGvk); ok {
			return []string{ownerName}
		}

		return []string{}
	})
	if err != nil {
		return err
	}

	err = mgr.GetFieldIndexer().IndexField(context.Background(), &runtimev1alpha1.Workload{}, workloadReplicaSetHashIndex, func(rawObj client.Object) []string {
		if _, ok := isOwnedByController(rawObj, replicaSetGvk); !ok {
			return []string{}
		}

		if generation, ok := rawObj.GetAnnotations()[workloadReplicaSetGenerationAnnotation]; ok {
			return []string{generation}
		}

		return []string{}
	})
	if err != nil {
		return err
	}

	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.WorkloadReplicaSet{}).
		Owns(&runtimev1alpha1.Workload{}).
		Named("workload-WorkloadReplicaSet").
		Complete(r)
}
