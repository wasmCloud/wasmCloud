package runtime

import (
	"context"
	"fmt"

	batchv1 "k8s.io/api/batch/v1"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// ArtifactStoreConfig decomplects the storage concern for precompile outputs.
// BaseURL selects the backend via its scheme (nats://, s3://, file://, ...).
// Env carries transport configuration (credentials, endpoints) keyed by scheme.
type ArtifactStoreConfig struct {
	BaseURL string
	Env     []corev1.EnvVar
}

// PrecompileReconciler creates a precompile Job per Artifact and reports the
// outcome via Artifact.Status.Precompiled plus Precompiled / PrecompileFailed /
// PrecompileProgressing conditions.
type PrecompileReconciler struct {
	client.Client
	Scheme          *runtime.Scheme
	WorkerImage     string
	ArtifactStore   ArtifactStoreConfig
	Target          string
	WasmtimeVersion string
}

// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=artifacts,verbs=get;list;watch
// +kubebuilder:rbac:groups=runtime.wasmcloud.dev,resources=artifacts/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=batch,resources=jobs,verbs=get;list;watch;create
// +kubebuilder:rbac:groups=batch,resources=jobs/status,verbs=get

func (r *PrecompileReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&runtimev1alpha1.Artifact{}).
		Owns(&batchv1.Job{}).
		Named("runtime-artifact-precompile").
		Complete(r)
}

func (r *PrecompileReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	var a runtimev1alpha1.Artifact
	if err := r.Get(ctx, req.NamespacedName, &a); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	outputURL := fmt.Sprintf("%s/%s/%s-%s.cwasm",
		r.ArtifactStore.BaseURL,
		a.Name,
		r.Target,
		r.WasmtimeVersion,
	)
	jobName := "precompile-" + a.Name

	desired := &batchv1.Job{
		ObjectMeta: metav1.ObjectMeta{
			Name:      jobName,
			Namespace: a.Namespace},
		Spec: batchv1.JobSpec{
			Template: corev1.PodTemplateSpec{
				Spec: corev1.PodSpec{
					RestartPolicy: corev1.RestartPolicyNever,
					Containers: []corev1.Container{{
						Name:  "precompile",
						Image: r.WorkerImage,
						Args: []string{
							"--image", a.Spec.Image,
							"--output", outputURL,
						},
						Env: r.ArtifactStore.Env,
					}},
				},
			},
		},
	}
	if err := controllerutil.SetControllerReference(&a, desired, r.Scheme); err != nil {
		return ctrl.Result{}, err
	}

	if err := r.Create(ctx, desired); err != nil && !apierrors.IsAlreadyExists(err) {
		return ctrl.Result{}, err
	}

	var job batchv1.Job
	if err := r.Get(ctx, types.NamespacedName{Namespace: a.Namespace, Name: jobName}, &job); err != nil {
		return ctrl.Result{}, err
	}

	if failed, msg := jobFailed(&job); failed {
		existing := a.Status.GetCondition(runtimev1alpha1.ArtifactConditionPrecompileFailed)
		if existing.Status == condition.ConditionTrue && existing.Message == msg {
			return ctrl.Result{}, nil
		}

		a.Status.SetConditions(
			condition.Condition{
				Type:               runtimev1alpha1.ArtifactConditionPrecompileFailed,
				Status:             condition.ConditionTrue,
				Reason:             "JobFailed",
				Message:            msg,
				LastTransitionTime: metav1.Now(),
			},
			condition.Condition{
				Type:               runtimev1alpha1.ArtifactConditionPrecompileProgressing,
				Status:             condition.ConditionFalse,
				Reason:             "JobFailed",
				LastTransitionTime: metav1.Now(),
			},
		)
		return ctrl.Result{}, r.Status().Update(ctx, &a)
	}

	if !jobComplete(&job) {
		existing := a.Status.GetCondition(runtimev1alpha1.ArtifactConditionPrecompileProgressing)
		if existing.Status == condition.ConditionTrue {
			return ctrl.Result{}, nil
		}

		a.Status.SetConditions(
			condition.Condition{
				Type:               runtimev1alpha1.ArtifactConditionPrecompileProgressing,
				Status:             condition.ConditionTrue,
				Reason:             "JobInFlight",
				LastTransitionTime: metav1.Now(),
			},
		)
		return ctrl.Result{}, r.Status().Update(ctx, &a)
	}

	variant := runtimev1alpha1.PrecompiledVariant{
		Target:          r.Target,
		WasmtimeVersion: r.WasmtimeVersion,
		ArtifactURL:     outputURL,
	}
	if variantRecorded(a.Status.Precompiled, variant) {
		return ctrl.Result{}, nil
	}
	a.Status.Precompiled = append(a.Status.Precompiled, variant)
	a.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.ArtifactConditionPrecompiled),
		condition.Condition{
			Type:               runtimev1alpha1.ArtifactConditionPrecompileProgressing,
			Status:             condition.ConditionFalse,
			Reason:             "JobSucceeded",
			LastTransitionTime: metav1.Now(),
		},
	)
	return ctrl.Result{}, r.Status().Update(ctx, &a)
}

func jobComplete(j *batchv1.Job) bool {
	for _, c := range j.Status.Conditions {
		if c.Type == batchv1.JobComplete && c.Status == corev1.ConditionTrue {
			return true
		}
	}
	return false
}

func jobFailed(j *batchv1.Job) (bool, string) {
	for _, c := range j.Status.Conditions {
		if c.Type == batchv1.JobFailed && c.Status == corev1.ConditionTrue {
			return true, c.Message
		}
	}
	return false, ""
}

func variantRecorded(existing []runtimev1alpha1.PrecompiledVariant, v runtimev1alpha1.PrecompiledVariant) bool {
	for _, e := range existing {
		if e.Target == v.Target && e.WasmtimeVersion == v.WasmtimeVersion {
			return true
		}
	}
	return false
}
