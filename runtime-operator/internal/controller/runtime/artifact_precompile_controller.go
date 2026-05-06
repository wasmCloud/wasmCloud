package runtime

import (
	"context"
	"fmt"
	"slices"

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

	desired, err := r.buildDesiredJob(&a)
	if err != nil {
		return ctrl.Result{}, err
	}

	if res, err, done := r.handleImageChange(ctx, &a, desired); done {
		return res, err
	}

	if err := r.Create(ctx, desired); err != nil && !apierrors.IsAlreadyExists(err) {
		return ctrl.Result{}, err
	}

	var job batchv1.Job
	if err := r.Get(ctx, types.NamespacedName{Namespace: a.Namespace, Name: desired.Name}, &job); err != nil {
		return ctrl.Result{}, err
	}

	if failed, msg := jobFailed(&job); failed {
		return r.handleFailedJob(ctx, &a, msg)
	}

	if !jobComplete(&job) {
		return r.handleInFlightJob(ctx, &a)
	}

	return r.handleSuccessfulJob(ctx, &a)
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

func argsMatch(existing, desired *batchv1.Job) bool {
	if len(existing.Spec.Template.Spec.Containers) == 0 ||
		len(desired.Spec.Template.Spec.Containers) == 0 {
		return false
	}
	return slices.Equal(
		existing.Spec.Template.Spec.Containers[0].Args,
		desired.Spec.Template.Spec.Containers[0].Args,
	)
}

func (r *PrecompileReconciler) outputURLOf(a *runtimev1alpha1.Artifact) string {
	return fmt.Sprintf("%s/%s/%s-%s.cwasm",
		r.ArtifactStore.BaseURL,
		a.Name,
		r.Target,
		r.WasmtimeVersion,
	)
}

func (r *PrecompileReconciler) buildDesiredJob(a *runtimev1alpha1.Artifact) (*batchv1.Job, error) {
	container := corev1.Container{
		Name:  "precompile",
		Image: r.WorkerImage,
		Args: []string{
			"--image", a.Spec.Image,
			"--output", r.outputURLOf(a),
		},
		Env: r.ArtifactStore.Env,
	}

	var volumes []corev1.Volume
	if a.Spec.ImagePullSecret != nil {
		container.Env = append(container.Env, corev1.EnvVar{
			Name:  "DOCKER_CONFIG",
			Value: "/etc/docker-creds",
		})
		container.VolumeMounts = append(container.VolumeMounts, corev1.VolumeMount{
			Name:      "docker-creds",
			MountPath: "/etc/docker-creds",
			ReadOnly:  true,
		})
		volumes = append(volumes, corev1.Volume{
			Name: "docker-creds",
			VolumeSource: corev1.VolumeSource{
				Secret: &corev1.SecretVolumeSource{
					SecretName: a.Spec.ImagePullSecret.Name,
					Items: []corev1.KeyToPath{{
						Key:  ".dockerconfigjson",
						Path: "config.json",
					}},
				},
			},
		})
	}

	job := &batchv1.Job{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "precompile-" + a.Name,
			Namespace: a.Namespace,
		},
		Spec: batchv1.JobSpec{
			Template: corev1.PodTemplateSpec{
				Spec: corev1.PodSpec{
					RestartPolicy: corev1.RestartPolicyNever,
					Containers:    []corev1.Container{container},
					Volumes:       volumes,
				},
			},
		},
	}
	if err := controllerutil.SetControllerReference(a, job, r.Scheme); err != nil {
		return nil, err
	}
	return job, nil
}

func (r *PrecompileReconciler) handleSuccessfulJob(
	ctx context.Context, a *runtimev1alpha1.Artifact,
) (ctrl.Result, error) {
	variant := runtimev1alpha1.PrecompiledVariant{
		Target:          r.Target,
		WasmtimeVersion: r.WasmtimeVersion,
		ArtifactURL:     r.outputURLOf(a),
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
	return ctrl.Result{}, r.Status().Update(ctx, a)
}

func (r *PrecompileReconciler) handleInFlightJob(
	ctx context.Context, a *runtimev1alpha1.Artifact,
) (ctrl.Result, error) {
	if a.Status.GetCondition(runtimev1alpha1.ArtifactConditionPrecompileProgressing).Status ==
		condition.ConditionTrue {
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
	return ctrl.Result{}, r.Status().Update(ctx, a)
}

func (r *PrecompileReconciler) handleFailedJob(
	ctx context.Context, a *runtimev1alpha1.Artifact, msg string,
) (ctrl.Result, error) {
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
	return ctrl.Result{}, r.Status().Update(ctx, a)
}

func (r *PrecompileReconciler) handleImageChange(
	ctx context.Context, a *runtimev1alpha1.Artifact, desired *batchv1.Job,
) (ctrl.Result, error, bool) {
	var existing batchv1.Job
	err := r.Get(ctx, types.NamespacedName{
		Name: desired.Name, Namespace: desired.Namespace,
	}, &existing)
	switch {
	case err == nil:
		if existing.DeletionTimestamp != nil {
			return ctrl.Result{}, nil, true
		}
		if !argsMatch(&existing, desired) {
			if delErr := r.Delete(ctx, &existing,
				client.PropagationPolicy(metav1.DeletePropagationBackground),
				client.GracePeriodSeconds(0),
			); delErr != nil && !apierrors.IsNotFound(delErr) {
				return ctrl.Result{}, delErr, true
			}
			a.Status.Precompiled = nil
			return ctrl.Result{}, r.Status().Update(ctx, a), true
		}
		return ctrl.Result{}, nil, false
	case !apierrors.IsNotFound(err):
		return ctrl.Result{}, err, true
	default:
		return ctrl.Result{}, nil, false
	}
}
