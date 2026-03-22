package runtime

import (
	"context"
	"fmt"
	"time"

	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"

	"go.wasmcloud.dev/runtime-operator/api/condition"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/api/runtime/v1alpha1"
)

const (
	artifactReconcileInterval = 5 * time.Minute
)

// ArtifactReconciler reconciles a Workload object
type ArtifactReconciler struct {
	client.Client
	Scheme *runtime.Scheme

	reconciler condition.AnyConditionedReconciler
}

func (r *ArtifactReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	return r.reconciler.Reconcile(ctx, req)
}

func (r *ArtifactReconciler) reconcileSync(ctx context.Context, artifact *runtimev1alpha1.Artifact) error {
	if artifact.Status.ObservedGeneration == artifact.Generation {
		return nil
	}

	condition.ForceStatusUpdate(ctx)
	artifact.Status.ObservedGeneration = artifact.Generation
	artifact.Status.ArtifactURL = artifact.Spec.Image

	// Skip remaining steps cause we touched the .Status field
	return condition.ErrSkipReconciliation()
}

func (r *ArtifactReconciler) reconcilePublished(_ context.Context, artifact *runtimev1alpha1.Artifact) error {
	if artifact.Status.ArtifactURL != "" {
		return nil
	}

	return condition.ErrStatusUnknown(fmt.Errorf("artifact not published"))
}

func (r *ArtifactReconciler) reconcileReady(_ context.Context, artifact *runtimev1alpha1.Artifact) error {
	if !artifact.Status.AllTrue(
		runtimev1alpha1.ArtifactConditionSync,
		runtimev1alpha1.ArtifactConditionPublished) {
		return fmt.Errorf("artifact is not ready")
	}
	return nil
}

// SetupWithManager sets up the controller with the Manager.
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=artifacts,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=artifacts/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=artifacts/finalizers,verbs=update

func (r *ArtifactReconciler) SetupWithManager(mgr ctrl.Manager) error {
	reconciler := condition.NewConditionedReconciler(
		r.Client,
		r.Scheme,
		&runtimev1alpha1.Artifact{},
		artifactReconcileInterval)

	reconciler.SetCondition(runtimev1alpha1.ArtifactConditionSync, r.reconcileSync)
	reconciler.SetCondition(runtimev1alpha1.ArtifactConditionPublished, r.reconcilePublished)
	reconciler.SetCondition(condition.TypeReady, r.reconcileReady)

	r.reconciler = reconciler

	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.Artifact{}).
		Named("runtime-artifact").
		Complete(r)
}
