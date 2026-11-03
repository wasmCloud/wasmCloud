package runtime

import (
	"context"
	"errors"
	"slices"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestArtifactReconcilePublishesImageUpdates(t *testing.T) {
	scheme := newScaleTestScheme(t)
	artifact := &runtimev1alpha1.Artifact{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: "component", Generation: 1},
		Spec:       runtimev1alpha1.ArtifactSpec{Image: "registry.example/component:v1"},
	}
	c := fake.NewClientBuilder().WithScheme(scheme).WithObjects(artifact).WithStatusSubresource(artifact).Build()
	conditioned := condition.NewConditionedReconciler(c, scheme, &runtimev1alpha1.Artifact{}, time.Minute)
	r := &ArtifactReconciler{Client: c, Scheme: scheme, reconciler: conditioned}
	conditioned.SetFinalizer(artifactFinalizerName, r.finalize)
	conditioned.SetCondition(runtimev1alpha1.ArtifactConditionSync, r.reconcileSync)
	conditioned.SetCondition(runtimev1alpha1.ArtifactConditionPublished, r.reconcilePublished)
	conditioned.SetCondition(condition.TypeReady, r.reconcileReady)
	key := client.ObjectKeyFromObject(artifact)
	for _, image := range []string{artifact.Spec.Image, "registry.example/component:v2"} {
		if artifact.Spec.Image != image {
			artifact.Spec.Image = image
			artifact.Generation++
			if err := c.Update(t.Context(), artifact); err != nil {
				t.Fatal(err)
			}
		}
		for range 5 {
			if _, err := r.Reconcile(t.Context(), ctrl.Request{NamespacedName: key}); err != nil {
				t.Fatal(err)
			}
		}
		if err := c.Get(t.Context(), key, artifact); err != nil {
			t.Fatal(err)
		}
		if artifact.Status.ArtifactURL != image || artifact.Status.ObservedGeneration != artifact.Generation {
			t.Fatalf("published artifact: %+v", artifact.Status)
		}
		if !artifact.Status.AllTrue(runtimev1alpha1.ArtifactConditionSync, runtimev1alpha1.ArtifactConditionPublished, condition.TypeReady) {
			t.Fatalf("artifact not ready: %+v", artifact.Status.Conditions)
		}
		if !slices.Contains(artifact.Finalizers, artifactFinalizerName) {
			t.Fatal("artifact has no deletion guard")
		}
	}
}

func TestArtifactPendingPublication(t *testing.T) {
	r := &ArtifactReconciler{}
	artifact := &runtimev1alpha1.Artifact{}
	if err := r.reconcilePublished(t.Context(), artifact); !condition.IsStatusUnknown(err) {
		t.Fatalf("unpublished artifact: %v", err)
	}
	if err := r.reconcileReady(t.Context(), artifact); err == nil {
		t.Fatal("unpublished artifact is ready")
	}
}

func TestArtifactFinalizationWaitsForLiveReferences(t *testing.T) {
	for _, state := range []string{"referenced", "unreferenced", "other namespace", "deleting deployment"} {
		t.Run(state, func(t *testing.T) {
			artifact := &runtimev1alpha1.Artifact{ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: "component"}}
			deployment := &runtimev1alpha1.WorkloadDeployment{
				ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: "consumer"},
				Spec: runtimev1alpha1.WorkloadDeploymentSpec{Artifacts: []runtimev1alpha1.WorkloadDeploymentArtifact{
					{ArtifactFrom: corev1.LocalObjectReference{Name: artifact.Name}},
				}},
			}
			switch state {
			case "unreferenced":
				deployment.Spec.Artifacts[0].ArtifactFrom.Name = "another-component"
			case "other namespace":
				deployment.Namespace = "other"
			case "deleting deployment":
				now := metav1.Now()
				deployment.DeletionTimestamp = &now
				deployment.Finalizers = []string{"other-controller"}
			}
			c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(deployment).Build()
			r := &ArtifactReconciler{Client: c}
			if err := r.finalize(t.Context(), artifact); (err != nil) != (state == "referenced") {
				t.Fatalf("finalize() = %v for %s deployment", err, state)
			}
		})
	}
	want := errors.New("API unavailable")
	c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithInterceptorFuncs(interceptor.Funcs{
		List: func(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error { return want },
	}).Build()
	r := &ArtifactReconciler{Client: c}
	if err := r.finalize(t.Context(), &runtimev1alpha1.Artifact{}); !errors.Is(err, want) {
		t.Fatalf("finalize() = %v, want %v", err, want)
	}
}
