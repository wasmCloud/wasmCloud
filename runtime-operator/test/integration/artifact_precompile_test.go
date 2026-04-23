package integration

import (
	"context"
	"path/filepath"
	"testing"
	"time"

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

const testWorkerImage = "ghcr.io/wasmcloud/wash:test"

var (
	testEnv   *envtest.Environment
	k8sClient client.Client
	cancelMgr context.CancelFunc
)

type precompileReconciler struct {
	client.Client
	Scheme      *runtime.Scheme
	WorkerImage string
}

func (r *precompileReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	var a runtimev1alpha1.Artifact
	if err := r.Get(ctx, req.NamespacedName, &a); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

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
		ctrl.NewControllerManagedBy(mgr).For(&runtimev1alpha1.Artifact{}).Complete(&precompileReconciler{Client: mgr.GetClient(), Scheme: mgr.GetScheme(), WorkerImage: testWorkerImage}),
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

var _ = Describe("Artifact CRD", func() {
	It("can be created and read back", func() {
		ctx := context.Background()

		a := &runtimev1alpha1.Artifact{ObjectMeta: metav1.ObjectMeta{Name: "demo", Namespace: "default"},
			Spec: runtimev1alpha1.ArtifactSpec{Image: "ghcr.io/example/comp:v1"},
		}
		Expect(k8sClient.Create(ctx, a)).To(Succeed())

		var got runtimev1alpha1.Artifact
		key := types.NamespacedName{Namespace: "default", Name: "demo"}
		Expect(k8sClient.Get(ctx, key, &got)).To(Succeed())
		Expect(got.Spec.Image).To(Equal("ghcr.io/example/comp:v1"))
	})
})

var _ = Describe("precompile pipeline", func() {
	It("creates a Job when an Artifact is created", func() {
		ctx := context.Background()
		a := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Name: "needs-precompile", Namespace: "default"},
			Spec:       runtimev1alpha1.ArtifactSpec{Image: "ghcr.io/example/comp:v1"},
		}

		Expect(k8sClient.Create(ctx, a)).To(Succeed())

		Eventually(func() int {
			var jobs batchv1.JobList
			if err := k8sClient.List(ctx, &jobs, client.InNamespace("default")); err != nil {
				return -1
			}
			count := 0
			for _, j := range jobs.Items {
				for _, o := range j.OwnerReferences {
					if o.UID == a.UID {
						count++
					}
				}
			}
			return count
		}, 10*time.Second, 250*time.Millisecond).Should(Equal(1))
	})
})

var _ = Describe("precompile Job spec", func() {
	It("uses the configured worker image", func() {
		ctx := context.Background()
		a := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Name: "img-check", Namespace: "default"},
			Spec:       runtimev1alpha1.ArtifactSpec{Image: "ghcr.io/example/comp:v1"},
		}

		Expect(k8sClient.Create(ctx, a)).To(Succeed())

		Eventually(func() string {
			var job batchv1.Job
			err := k8sClient.Get(ctx, types.NamespacedName{
				Namespace: "default", Name: "precompile-img-check"}, &job)

			if err != nil || len(job.Spec.Template.Spec.Containers) == 0 {
				return ""
			}
			return job.Spec.Template.Spec.Containers[0].Image
		}).Should(Equal(testWorkerImage))

	})
})
