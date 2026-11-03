package runtime

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	artifactNoReferencesCase     = "none"
	artifactReadErrorCase        = "read error"
	artifactNoServiceCase        = "no service"
	artifactUnknownComponentCase = "unknown component"
	artifactUnknownServiceCase   = "unknown service"
	artifactMissingObjectCase    = "missing object"

	artifactDirectServiceCase = "direct service"
	artifactMismatchCase      = "mismatch"
	componentArtifactAlias    = "component"
	serviceArtifactAlias      = "service"
	componentArtifactURL      = "oci://registry.example/component@sha256:1234"
	serviceArtifactURL        = "oci://registry.example/service@sha256:5678"
	directImageURL            = "registry.example/direct:latest"
)

func deploymentArtifactFixtures() ([]runtimev1alpha1.WorkloadDeploymentArtifact, []client.Object) {
	refs := []runtimev1alpha1.WorkloadDeploymentArtifact{
		{Name: componentArtifactAlias, ArtifactFrom: corev1.LocalObjectReference{Name: "component-build"}},
		{Name: serviceArtifactAlias, ArtifactFrom: corev1.LocalObjectReference{Name: "service-build"}},
	}
	objects := make([]client.Object, 0, len(refs))
	for i, url := range []string{componentArtifactURL, serviceArtifactURL} {
		artifact := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: refs[i].ArtifactFrom.Name},
			Status:     runtimev1alpha1.ArtifactStatus{ArtifactURL: url},
		}
		artifact.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.ArtifactConditionPublished))
		objects = append(objects, artifact)
	}
	return refs, objects
}

func artifactTemplate() runtimev1alpha1.WorkloadReplicaTemplate {
	return runtimev1alpha1.WorkloadReplicaTemplate{Spec: runtimev1alpha1.WorkloadSpec{
		Components: []runtimev1alpha1.WorkloadComponent{
			{Name: "from-artifact", Image: "artifact://" + componentArtifactAlias},
			{Name: "direct", Image: directImageURL},
		},
		Service: &runtimev1alpha1.WorkloadService{Image: "artifact://" + serviceArtifactAlias},
	}}
}

func TestControllersWaitForPublishedArtifacts(t *testing.T) {
	readError := errors.New("artifact read failed")
	for _, state := range []string{artifactNoReferencesCase, "published", "missing reference", "unpublished", "artifact in another namespace", artifactReadErrorCase} {
		t.Run(state, func(t *testing.T) {
			refs, objects := deploymentArtifactFixtures()
			switch state {
			case artifactNoReferencesCase:
				refs = nil
			case "missing reference":
				objects = objects[:1]
			case "unpublished":
				objects[1].(*runtimev1alpha1.Artifact).Status.SetConditions(condition.UnknownCondition(
					runtimev1alpha1.ArtifactConditionPublished, "Pending", "upload in progress"))
			case "artifact in another namespace":
				objects[1].SetNamespace("artifact-elsewhere")
			}
			funcs := interceptor.Funcs{}
			if state == artifactReadErrorCase {
				funcs.Get = func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
					return readError
				}
			}
			r := newScaleReconcilerWith(t, funcs, objects...)
			deployment := readyDeployment("")
			deployment.Spec.Artifacts = refs
			daemonSet := &runtimev1alpha1.WorkloadDaemonSet{
				ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace},
				Spec:       runtimev1alpha1.WorkloadDaemonSetSpec{Artifacts: refs},
			}
			d := &WorkloadDaemonSetReconciler{Client: r.Client}
			for _, err := range []error{r.reconcileArtifacts(t.Context(), deployment), d.reconcileArtifacts(t.Context(), daemonSet)} {
				switch state {
				case artifactNoReferencesCase, "published":
					if err != nil {
						t.Fatalf("artifacts ready: %v", err)
					}
				case artifactReadErrorCase:
					if !errors.Is(err, readError) || condition.IsStatusUnknown(err) {
						t.Fatalf("read error = %v, want %v", err, readError)
					}
				default:
					if !condition.IsStatusUnknown(err) || !strings.Contains(err.Error(), refs[1].ArtifactFrom.Name) {
						t.Fatalf("unready artifact = %v, want Unknown naming the second artifact", err)
					}
				}
			}
		})
	}
}

func TestResolveArtifacts(t *testing.T) {
	for _, state := range []string{"both", artifactNoServiceCase, artifactDirectServiceCase, artifactUnknownComponentCase, artifactUnknownServiceCase, artifactMissingObjectCase, artifactReadErrorCase} {
		t.Run(state, func(t *testing.T) {
			refs, objects := deploymentArtifactFixtures()
			tpl := artifactTemplate()
			switch state {
			case artifactNoServiceCase:
				tpl.Spec.Service = nil
			case artifactDirectServiceCase:
				tpl.Spec.Service.Image = directImageURL
			case artifactUnknownComponentCase:
				tpl.Spec.Components[0].Image = "artifact://unknown"
			case artifactUnknownServiceCase:
				tpl.Spec.Service.Image = "artifact://unknown"
			case artifactMissingObjectCase:
				objects = objects[:1]
			}
			readError := errors.New("artifact read failed")
			funcs := interceptor.Funcs{}
			if state == artifactReadErrorCase {
				funcs.Get = func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
					return readError
				}
			}
			r := newScaleReconcilerWith(t, funcs, objects...)
			err := resolveArtifacts(t.Context(), r.Client, testNamespace, &tpl, refs)
			switch state {
			case artifactUnknownComponentCase, artifactUnknownServiceCase:
				if err == nil || condition.IsStatusUnknown(err) || !strings.Contains(err.Error(), "artifact unknown not found in deployment spec") {
					t.Fatalf("undeclared artifact = %v", err)
				}
			case artifactMissingObjectCase:
				if !condition.IsStatusUnknown(err) || !strings.Contains(err.Error(), refs[1].ArtifactFrom.Name) {
					t.Fatalf("missing artifact = %v", err)
				}
			case artifactReadErrorCase:
				if !errors.Is(err, readError) {
					t.Fatalf("read error = %v, want %v", err, readError)
				}
			default:
				if err != nil {
					t.Fatal(err)
				}
				if tpl.Spec.Components[0].Image != componentArtifactURL || tpl.Spec.Components[1].Image != directImageURL {
					t.Fatalf("resolved components = %+v", tpl.Spec.Components)
				}
				if state != artifactNoServiceCase {
					want := serviceArtifactURL
					if state == artifactDirectServiceCase {
						want = directImageURL
					}
					if tpl.Spec.Service.Image != want {
						t.Fatalf("service image = %q, want %q", tpl.Spec.Service.Image, want)
					}
				}
			}
		})
	}
}

func TestDeploymentSyncUsesResolvedTemplate(t *testing.T) {
	for _, state := range []string{"matched", "no artifacts", "no active set", "missing active set", artifactReadErrorCase, "missing artifact", artifactMismatchCase} {
		t.Run(state, func(t *testing.T) {
			refs, objects := deploymentArtifactFixtures()
			deployment := readyDeployment("")
			deployment.Spec.Artifacts = refs
			deployment.Spec.Template = artifactTemplate()
			deployment.Spec.Kubernetes = &runtimev1alpha1.KubernetesSpec{Service: &runtimev1alpha1.KubernetesServiceRef{Name: "public"}}
			deployment.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact))
			rs := baseReplicaSet(testReplicaSetName)
			rs.Spec.Template = artifactTemplate()
			rs.Spec.Template.Spec.Components[0].Image = componentArtifactURL
			rs.Spec.Template.Spec.Service.Image = serviceArtifactURL
			rs.Spec.Template.Spec.Kubernetes = deployment.Spec.Kubernetes.DeepCopy()
			switch state {
			case "no artifacts":
				deployment.Status.SetConditions(condition.UnknownCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact, "Pending", "waiting"))
			case "no active set":
				deployment.Status.CurrentReplicaSet = nil
			case "missing artifact":
				objects = objects[:1]
			case artifactMismatchCase:
				rs.Spec.Template.Spec.Service.Image = directImageURL
			}
			if state != "missing active set" {
				objects = append(objects, rs)
			}
			readError := errors.New("replica set read failed")
			funcs := interceptor.Funcs{}
			if state == artifactReadErrorCase {
				funcs.Get = func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
					return readError
				}
			}
			r := newScaleReconcilerWith(t, funcs, objects...)
			before := deployment.DeepCopy()
			err := r.reconcileSync(t.Context(), deployment)
			switch state {
			case "matched":
				if err != nil {
					t.Fatal(err)
				}
			case artifactMismatchCase:
				if err == nil || condition.IsStatusUnknown(err) || !strings.Contains(err.Error(), "template hash mismatch") {
					t.Fatalf("mismatched template = %v", err)
				}
			case artifactReadErrorCase:
				if !errors.Is(err, readError) {
					t.Fatalf("read error = %v, want %v", err, readError)
				}
			default:
				if !condition.IsStatusUnknown(err) {
					t.Fatalf("incomplete rollout = %v, want Unknown", err)
				}
			}
			if !reflect.DeepEqual(deployment.Spec, before.Spec) {
				t.Fatal("sync mutated the deployment spec")
			}
		})
	}
}

func TestDeploymentRolloutResolvesArtifactsWithoutMutatingSpec(t *testing.T) {
	for _, policy := range []runtimev1alpha1.WorkloadDeployPolicy{"", runtimev1alpha1.WorkloadDeployPolicyRollingUpdate, runtimev1alpha1.WorkloadDeployPolicyRecreate} {
		t.Run(string(policy), func(t *testing.T) {
			refs, objects := deploymentArtifactFixtures()
			deployment := readyDeployment("")
			deployment.Spec.DeployPolicy = policy
			deployment.Spec.Artifacts = refs
			deployment.Spec.Template = artifactTemplate()
			deployment.Spec.Kubernetes = &runtimev1alpha1.KubernetesSpec{Service: &runtimev1alpha1.KubernetesServiceRef{Name: "public"}}
			deployment.Labels = map[string]string{"app": testDeploymentName}
			deployment.Annotations = map[string]string{"release": "v2"}
			deployment.Status.SetConditions(
				condition.ReadyCondition(runtimev1alpha1.WorkloadDeploymentConditionArtifact),
				condition.UnknownCondition(runtimev1alpha1.WorkloadDeploymentConditionSync, "Pending", "template changed"),
			)
			r := newScaleReconciler(t, objects...)
			before := deployment.DeepCopy()
			if err := r.reconcileDeploy(t.Context(), deployment); !errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatalf("deploy = %v, want status update", err)
			}
			sets := &runtimev1alpha1.WorkloadReplicaSetList{}
			if err := r.List(t.Context(), sets); err != nil {
				t.Fatal(err)
			}
			if len(sets.Items) != 1 {
				t.Fatalf("created %d ReplicaSets, want one", len(sets.Items))
			}
			created := &sets.Items[0]
			if deployment.Status.CurrentReplicaSet == nil || deployment.Status.CurrentReplicaSet.Name != created.Name {
				t.Fatalf("current ReplicaSet = %+v, created %q", deployment.Status.CurrentReplicaSet, created.Name)
			}
			wantPrevious := before.Status.CurrentReplicaSet
			if policy == runtimev1alpha1.WorkloadDeployPolicyRecreate {
				wantPrevious = nil
			}
			if !reflect.DeepEqual(deployment.Status.PreviousReplicaSet, wantPrevious) {
				t.Fatalf("previous ReplicaSet = %+v, want %+v", deployment.Status.PreviousReplicaSet, wantPrevious)
			}
			wantSpec := before.Spec.WorkloadReplicaSetSpec.DeepCopy()
			wantSpec.Template.Spec.Components[0].Image = componentArtifactURL
			wantSpec.Template.Spec.Service.Image = serviceArtifactURL
			wantSpec.Template.Spec.Kubernetes = before.Spec.Kubernetes.DeepCopy()
			if !reflect.DeepEqual(created.Spec, *wantSpec) {
				t.Fatalf("created spec = %+v, want %+v", created.Spec, *wantSpec)
			}
			if created.Namespace != deployment.Namespace || !metav1.IsControlledBy(created, deployment) {
				t.Fatalf("ReplicaSet metadata = %+v", created.ObjectMeta)
			}
			if created.Labels["app"] != testDeploymentName || created.Labels[workloadDeploymentNameLabel] != deployment.Name || !reflect.DeepEqual(created.Annotations, before.Annotations) {
				t.Fatalf("ReplicaSet labels or annotations = %+v", created.ObjectMeta)
			}
			if !reflect.DeepEqual(deployment.Spec, before.Spec) || !reflect.DeepEqual(deployment.Labels, before.Labels) {
				t.Fatal("deploy mutated the source spec or labels")
			}
		})
	}
}
