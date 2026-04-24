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
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/envtest"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	testWorkerImage     = "ghcr.io/wasmcloud/wash:test"
	testArtifactImage   = "ghcr.io/example/comp:v1"
	testTarget          = "x86_64-unknown-linux-gnu"
	testWasmtimeVersion = "27.0.0"
)

type ArtifactStoreConfig struct {
	BaseURL string
	Env     []corev1.EnvVar
}

var testArtifactStore = ArtifactStoreConfig{
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

type precompileReconciler struct {
	client.Client
	Scheme          *runtime.Scheme
	WorkerImage     string
	ArtifactStore   ArtifactStoreConfig
	Target          string
	WasmtimeVersion string
}

func (r *precompileReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
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

	job := &batchv1.Job{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "precompile-" + a.Name,
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
	if err := controllerutil.SetControllerReference(&a, job, r.Scheme); err != nil {
		return ctrl.Result{}, err
	}

	if err := r.Create(ctx, job); err != nil && !apierrors.IsAlreadyExists(err) {
		return ctrl.Result{}, err
	}
	return ctrl.Result{}, nil
}

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

	Expect(
		ctrl.NewControllerManagedBy(mgr).For(&runtimev1alpha1.Artifact{}).Complete(&precompileReconciler{
			Client:          mgr.GetClient(),
			Scheme:          mgr.GetScheme(),
			WorkerImage:     testWorkerImage,
			ArtifactStore:   testArtifactStore,
			Target:          testTarget,
			WasmtimeVersion: testWasmtimeVersion,
		}),
	).To(Succeed())

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

			for _, want := range testArtifactStore.Env {
				g.Expect(c.Env).To(ContainElement(want))
			}
		}).Should(Succeed())

	})

})
