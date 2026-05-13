package runtime

import (
	"context"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

var _ = Describe("workloadDeploymentsReferencing", func() {
	It("returns WorkloadDeployments that reference the Artifact by name", func() {
		ctx := context.Background()
		art := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Name: "comp", Namespace: "default"},
		}
		referencing := &runtimev1alpha1.WorkloadDeployment{
			ObjectMeta: metav1.ObjectMeta{Name: "echo", Namespace: "default"},
			Spec: runtimev1alpha1.WorkloadDeploymentSpec{
				Artifacts: []runtimev1alpha1.WorkloadDeploymentArtifact{
					{Name: "comp", ArtifactFrom: corev1.LocalObjectReference{Name: "comp"}},
				},
			},
		}
		unrelated := &runtimev1alpha1.WorkloadDeployment{
			ObjectMeta: metav1.ObjectMeta{Name: "other", Namespace: "default"},
			Spec: runtimev1alpha1.WorkloadDeploymentSpec{
				Artifacts: []runtimev1alpha1.WorkloadDeploymentArtifact{
					{Name: "different", ArtifactFrom: corev1.LocalObjectReference{Name: "different"}},
				},
			},
		}
		r := &WorkloadDeploymentReconciler{Client: newTestClient(referencing, unrelated)}

		wds, err := r.workloadDeploymentsReferencing(ctx, art)

		Expect(err).ToNot(HaveOccurred())
		Expect(wds).To(HaveLen(1))
		Expect(wds[0].Name).To(Equal("echo"))
	})

	It("does not return WorkloadDeployments in a different namespace, even if they reference an Artifact with the same name", func() {
		ctx := context.Background()
		art := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Name: "comp", Namespace: "ns-a"},
		}
		otherNamespace := &runtimev1alpha1.WorkloadDeployment{
			ObjectMeta: metav1.ObjectMeta{Name: "echo", Namespace: "ns-b"},
			Spec: runtimev1alpha1.WorkloadDeploymentSpec{
				Artifacts: []runtimev1alpha1.WorkloadDeploymentArtifact{
					{Name: "comp", ArtifactFrom: corev1.LocalObjectReference{Name: "comp"}},
				},
			},
		}
		r := &WorkloadDeploymentReconciler{Client: newTestClient(otherNamespace)}

		wds, err := r.workloadDeploymentsReferencing(ctx, art)

		Expect(err).ToNot(HaveOccurred())
		Expect(wds).To(BeEmpty())
	})

	It("matches a WorkloadDeployment even when the referencing entry is not the first in Spec.Artifacts", func() {
		ctx := context.Background()
		art := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Name: "comp", Namespace: "default"},
		}
		wd := &runtimev1alpha1.WorkloadDeployment{
			ObjectMeta: metav1.ObjectMeta{Name: "echo", Namespace: "default"},
			Spec: runtimev1alpha1.WorkloadDeploymentSpec{
				Artifacts: []runtimev1alpha1.WorkloadDeploymentArtifact{
					{Name: "first", ArtifactFrom: corev1.LocalObjectReference{Name: "first"}},
					{Name: "comp", ArtifactFrom: corev1.LocalObjectReference{Name: "comp"}},
					{Name: "third", ArtifactFrom: corev1.LocalObjectReference{Name: "third"}},
				},
			},
		}
		r := &WorkloadDeploymentReconciler{Client: newTestClient(wd)}

		wds, err := r.workloadDeploymentsReferencing(ctx, art)

		Expect(err).ToNot(HaveOccurred())
		Expect(wds).To(HaveLen(1))
		Expect(wds[0].Name).To(Equal("echo"))
	})
})
