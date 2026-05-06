package integration

import (
	"context"
	"fmt"
	"path/filepath"
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	batchv1 "k8s.io/api/batch/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimectrl "go.wasmcloud.dev/runtime-operator/v2/internal/controller/runtime"
)

const (
	testWorkerImage     = "ghcr.io/wasmcloud/wash:test"
	testArtifactImage   = "ghcr.io/example/comp:v1"
	testTarget          = "x86_64-unknown-linux-gnu"
	testWasmtimeVersion = "27.0.0"
)

var testArtifactStore = runtimectrl.ArtifactStoreConfig{
	BaseURL: "nats://precompiled-artifacts",
	Env: []corev1.EnvVar{
		{Name: "NATS_URL", Value: "nats://test-nats:4222"},
	},
}

var (
	testEnv   *envtest.Environment
	k8sClient client.Client
	cancelMgr context.CancelFunc
)

func TestIntegration(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "Integration Suite")
}

var _ = BeforeSuite(func() {
	testEnv = &envtest.Environment{
		CRDDirectoryPaths:     []string{filepath.Join("..", "..", "config", "crd", "bases")},
		ErrorIfCRDPathMissing: true,
	}
	cfg, err := testEnv.Start()
	Expect(err).NotTo(HaveOccurred())

	scheme := runtime.NewScheme()
	Expect(clientgoscheme.AddToScheme(scheme)).To(Succeed())
	Expect(runtimev1alpha1.AddToScheme(scheme)).To(Succeed())

	k8sClient, err = client.New(cfg, client.Options{Scheme: scheme})
	Expect(err).NotTo(HaveOccurred())

	mgr, err := ctrl.NewManager(cfg, ctrl.Options{Scheme: scheme})
	Expect(err).NotTo(HaveOccurred())

	Expect((&runtimectrl.PrecompileReconciler{
		Client:          mgr.GetClient(),
		Scheme:          mgr.GetScheme(),
		WorkerImage:     testWorkerImage,
		ArtifactStore:   testArtifactStore,
		Target:          testTarget,
		WasmtimeVersion: testWasmtimeVersion,
	}).SetupWithManager(mgr)).To(Succeed())

	var mgrCtx context.Context
	mgrCtx, cancelMgr = context.WithCancel(context.Background())
	go func() {
		defer GinkgoRecover()
		Expect(mgr.Start(mgrCtx)).To(Succeed())
	}()
})

var _ = AfterSuite(func() {
	if cancelMgr != nil {
		cancelMgr()
	}
	Expect(testEnv.Stop()).To(Succeed())
})

func newArtifact(ctx context.Context, name string) *runtimev1alpha1.Artifact {
	GinkgoHelper()
	a := &runtimev1alpha1.Artifact{ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: "default"},
		Spec: runtimev1alpha1.ArtifactSpec{Image: testArtifactImage},
	}
	Expect(k8sClient.Create(ctx, a)).To(Succeed())
	return a
}

var _ = Describe("Artifact CRD", func() {
	It("can be created and read back", func() {
		ctx := context.Background()

		_ = newArtifact(ctx, "demo")

		var got runtimev1alpha1.Artifact
		key := types.NamespacedName{Namespace: "default", Name: "demo"}
		Expect(k8sClient.Get(ctx, key, &got)).To(Succeed())
		Expect(got.Spec.Image).To(Equal("ghcr.io/example/comp:v1"))
	})
})

var _ = Describe("precompile pipeline", func() {
	It("emits a Job that matches the precompile contract", func() {
		ctx := context.Background()
		a := newArtifact(ctx, "img-check")

		expectedUrl := fmt.Sprintf("%s/%s/%s-%s.cwasm",
			testArtifactStore.BaseURL, a.Name, testTarget, testWasmtimeVersion)

		Eventually(func(g Gomega) {
			var job batchv1.Job
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{Namespace: "default", Name: "precompile-" + a.Name}, &job)).To(Succeed())

			g.Expect(job.OwnerReferences).To(ContainElement(HaveField("UID", a.UID)))
			g.Expect(job.Spec.Template.Spec.Containers).To(HaveLen(1))

			c := job.Spec.Template.Spec.Containers[0]
			g.Expect(c.Image).To(Equal(testWorkerImage))
			g.Expect(c.Args).To(Equal([]string{
				"--image", a.Spec.Image,
				"--output", expectedUrl,
			}))

			g.Expect(job.Spec.Template.Spec.Volumes).To(BeEmpty())
			g.Expect(c.VolumeMounts).To(BeEmpty())
			g.Expect(c.Env).NotTo(ContainElement(HaveField("Name", "DOCKER_CONFIG")))

			for _, want := range testArtifactStore.Env {
				g.Expect(c.Env).To(ContainElement(want))
			}
		}).Should(Succeed())

	})

	It("reports successful precompilation in status when the Job succeeds", func() {
		ctx := context.Background()
		a := newArtifact(ctx, "populates-status")

		var job batchv1.Job
		Eventually(func(g Gomega) {
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: "precompile-" + a.Name,
			}, &job)).To(Succeed())
		}).Should(Succeed())

		job.Status.Succeeded = 1
		job.Status.Conditions = []batchv1.JobCondition{{
			Type:   batchv1.JobComplete,
			Status: corev1.ConditionTrue,
		}}
		Expect(k8sClient.Status().Update(ctx, &job)).To(Succeed())

		expectedURL := fmt.Sprintf("%s/%s/%s-%s.cwasm",
			testArtifactStore.BaseURL, a.Name, testTarget, testWasmtimeVersion)

		Eventually(func(g Gomega) {
			var got runtimev1alpha1.Artifact
			g.Expect(k8sClient.Get(ctx,
				types.NamespacedName{
					Namespace: "default", Name: a.Name,
				}, &got)).To(Succeed())

			g.Expect(got.Status.Precompiled).To(HaveLen(1))
			g.Expect(got.Status.Precompiled[0].ArtifactURL).To(Equal(expectedURL))
			g.Expect(got.Status.Precompiled[0].Target).To(Equal(testTarget))
			g.Expect(got.Status.Precompiled[0].WasmtimeVersion).To(Equal(testWasmtimeVersion))

			cond := got.Status.GetCondition(runtimev1alpha1.ArtifactConditionPrecompiled)
			g.Expect(cond.Status).To(Equal(corev1.ConditionTrue))

		},
		).Should(Succeed())

	})
	It("sets PrecompileFailed when the Job fails", func() {
		ctx := context.Background()
		a := newArtifact(ctx, "precompile-failed")

		var job batchv1.Job
		Eventually(func(g Gomega) {
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: "precompile-" + a.Name,
			}, &job)).To(Succeed())
		}).Should(Succeed())

		job.Status.Failed = 1
		job.Status.Conditions = []batchv1.JobCondition{{
			Type:    batchv1.JobFailed,
			Status:  corev1.ConditionTrue,
			Reason:  "Test reason",
			Message: "Test Message",
		}}
		Expect(k8sClient.Status().Update(ctx, &job)).To(Succeed())

		Eventually(func(g Gomega) {
			var got runtimev1alpha1.Artifact
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: a.Name,
			}, &got)).To(Succeed())

			failed := got.Status.GetCondition(runtimev1alpha1.ArtifactConditionPrecompileFailed)
			g.Expect(failed.Status).To(Equal(corev1.ConditionTrue))

			g.Expect(got.Status.Precompiled).To(BeEmpty())
		}).Should(Succeed())
	})

	It("sets PrecompileProgressing while the Job is in flight, and False on completion", func() {
		ctx := context.Background()
		a := newArtifact(ctx, "progressing")

		var job batchv1.Job
		Eventually(func(g Gomega) {
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: "precompile-" + a.Name,
			}, &job)).To(Succeed())
		}).Should(Succeed())

		Eventually(func(g Gomega) {
			var got runtimev1alpha1.Artifact
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: a.Name,
			}, &got)).To(Succeed())

			prog := got.Status.GetCondition(runtimev1alpha1.ArtifactConditionPrecompileProgressing)
			g.Expect(prog.Status).To(Equal(corev1.ConditionTrue))
		}).Should(Succeed())

		job.Status.Succeeded = 1
		job.Status.Conditions = []batchv1.JobCondition{{
			Type:   batchv1.JobComplete,
			Status: corev1.ConditionTrue,
		}}
		Expect(k8sClient.Status().Update(ctx, &job)).To(Succeed())

		Eventually(func(g Gomega) {
			var got runtimev1alpha1.Artifact
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: a.Name,
			}, &got)).To(Succeed())

			prog := got.Status.GetCondition(runtimev1alpha1.ArtifactConditionPrecompileProgressing)
			g.Expect(prog.Status).To(Equal(corev1.ConditionFalse))
		}).Should(Succeed())
	})

	It("mounts the imagePullSecret as a docker config when set", func() {
		ctx := context.Background()

		a := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Name: "needs-creds", Namespace: "default"},
			Spec: runtimev1alpha1.ArtifactSpec{
				Image:           testArtifactImage,
				ImagePullSecret: &corev1.LocalObjectReference{Name: "ghcr-secret"},
			},
		}
		Expect(k8sClient.Create(ctx, a)).To(Succeed())

		Eventually(func(g Gomega) {
			var job batchv1.Job
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: "precompile-" + a.Name,
			}, &job)).To(Succeed())

			g.Expect(job.Spec.Template.Spec.Volumes).To(HaveLen(1))
			vol := job.Spec.Template.Spec.Volumes[0]
			g.Expect(vol.Name).To(Equal("docker-creds"))
			g.Expect(vol.Secret).NotTo(BeNil())
			g.Expect(vol.Secret.SecretName).To(Equal("ghcr-secret"))
			g.Expect(vol.Secret.Items).To(ContainElement(corev1.KeyToPath{
				Key:  ".dockerconfigjson",
				Path: "config.json",
			}))

			c := job.Spec.Template.Spec.Containers[0]
			g.Expect(c.VolumeMounts).To(ContainElement(corev1.VolumeMount{
				Name:      "docker-creds",
				MountPath: "/etc/docker-creds",
				ReadOnly:  true,
			}))
			g.Expect(c.Env).To(ContainElement(corev1.EnvVar{
				Name:  "DOCKER_CONFIG",
				Value: "/etc/docker-creds",
			}))
		}).Should(Succeed())
	})

	It("re-runs precompile when Artifact.spec.image changes", func() {
		ctx := context.Background()
		a := newArtifact(ctx, "image-update")

		var oldJob batchv1.Job
		Eventually(func(g Gomega) {
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: "precompile-" + a.Name,
			}, &oldJob)).To(Succeed())
			g.Expect(oldJob.Spec.Template.Spec.Containers[0].Args).
				To(ContainElement(testArtifactImage))
		}).Should(Succeed())

		var updated runtimev1alpha1.Artifact
		Expect(k8sClient.Get(ctx, types.NamespacedName{
			Namespace: "default", Name: a.Name,
		}, &updated)).To(Succeed())
		updated.Spec.Image = "ghcr.io/example/comp:v2"
		Expect(k8sClient.Update(ctx, &updated)).To(Succeed())

		Eventually(func(g Gomega) {
			var newJob batchv1.Job
			g.Expect(k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: "precompile-" + a.Name,
			}, &newJob)).To(Succeed())
			g.Expect(newJob.UID).NotTo(Equal(oldJob.UID),
				"Job should have been recreated, not reused")
			g.Expect(newJob.Spec.Template.Spec.Containers[0].Args).
				To(ContainElement("ghcr.io/example/comp:v2"))
		}).Should(Succeed())
	})

})
