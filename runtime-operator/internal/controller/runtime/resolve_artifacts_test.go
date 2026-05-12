package runtime

import (
	"context"
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

var testScheme *runtime.Scheme

func TestRuntimeControllers(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "Runtime Controllers Suite")
}

var _ = BeforeSuite(func() {
	testScheme = runtime.NewScheme()
	Expect(runtimev1alpha1.AddToScheme(testScheme)).To(Succeed())
})

func newTestClient(objs ...client.Object) client.Client {
	GinkgoHelper()
	return fake.NewClientBuilder().WithScheme(testScheme).WithObjects(objs...).Build()
}

func newArtifact() *runtimev1alpha1.Artifact {
	return &runtimev1alpha1.Artifact{
		ObjectMeta: metav1.ObjectMeta{Name: "comp", Namespace: "default"},
		Spec:       runtimev1alpha1.ArtifactSpec{Image: "ghcr.io/x/y:v1"},
		Status: runtimev1alpha1.ArtifactStatus{
			ArtifactURL: "ghcr.io/x/y:v1",
			Precompiled: []runtimev1alpha1.PrecompiledVariant{{
				Target:          "x86_64-unknown-linux-gnu",
				WasmtimeVersion: "27.0.0",
				ArtifactURL:     "nats://store/comp/x86_64-unknown-linux-gnu-27.0.0.cwasm",
				ImageRef:        "ghcr.io/x/y:v1",
			}},
		},
	}
}

func newWorkloadReplicaTemplate() *runtimev1alpha1.WorkloadReplicaTemplate {
	return &runtimev1alpha1.WorkloadReplicaTemplate{
		Spec: runtimev1alpha1.WorkloadSpec{
			Components: []runtimev1alpha1.WorkloadComponent{{
				Name:  "main",
				Image: "artifact://comp",
			}},
		},
	}
}

func newWorkloadDeploymentArtifact() []runtimev1alpha1.WorkloadDeploymentArtifact {
	return []runtimev1alpha1.WorkloadDeploymentArtifact{
		{Name: "comp", ArtifactFrom: corev1.LocalObjectReference{Name: "comp"}},
	}
}

var _ = Describe("resolveArtifacts", func() {
	It("substitutes Image and leaves PrecompiledURL empty when precompile is disabled", func() {
		ctx := context.Background()
		c := newTestClient(newArtifact())
		tpl := newWorkloadReplicaTemplate()

		Expect(resolveArtifacts(ctx, c, "default", tpl, newWorkloadDeploymentArtifact(), nil)).To(Succeed())

		got := tpl.Spec.Components[0]
		Expect(got.Image).To(Equal("ghcr.io/x/y:v1"))
		Expect(got.PrecompiledURL).To(BeEmpty())
	})

	It("sets PrecompiledURL and substitutes Image when a matching precompile variant exists", func() {
		ctx := context.Background()
		c := newTestClient(newArtifact())
		tpl := newWorkloadReplicaTemplate()
		pc := &precompileMatch{
			Target:          "x86_64-unknown-linux-gnu",
			WasmtimeVersion: "27.0.0",
		}

		Expect(resolveArtifacts(ctx, c, "default", tpl, newWorkloadDeploymentArtifact(), pc)).To(Succeed())

		got := tpl.Spec.Components[0]
		Expect(got.PrecompiledURL).To(Equal("nats://store/comp/x86_64-unknown-linux-gnu-27.0.0.cwasm"))
		Expect(got.Image).To(Equal("ghcr.io/x/y:v1"))
	})

	It("returns status unknown to gate the deployment when no precompile variant matches", func() {
		ctx := context.Background()
		c := newTestClient(newArtifact())
		tpl := newWorkloadReplicaTemplate()
		pc := &precompileMatch{
			Target: "aarch64-apple-darwin", WasmtimeVersion: "27.0.0",
		}

		err := resolveArtifacts(ctx, c, "default", tpl, newWorkloadDeploymentArtifact(), pc)
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("status unknown"))
		Expect(err.Error()).To(ContainSubstring("no precompiled variant matching"))
	})

	It("returns status unknown when the only variant is for an old image", func() {
		ctx := context.Background()
		art := newArtifact()

		art.Spec.Image = "ghcr.io/x/y:v2"
		art.Status.ArtifactURL = "ghcr.io/x/y:v2"
		art.Status.Precompiled[0].ImageRef = "ghcr.io/x/y:v1"
		c := newTestClient(art)
		tpl := newWorkloadReplicaTemplate()
		pc := &precompileMatch{
			Target:          "x86_64-unknown-linux-gnu",
			WasmtimeVersion: "27.0.0",
		}

		err := resolveArtifacts(ctx, c, "default", tpl, newWorkloadDeploymentArtifact(), pc)
		Expect(err).To(HaveOccurred())
		Expect(err.Error()).To(ContainSubstring("status unknown"))
		Expect(err.Error()).To(ContainSubstring("no precompiled variant matching"))
	})
})
