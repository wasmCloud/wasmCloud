package integration

import (
	"context"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

var (
	testEnv        *envtest.Environment
	k8sClient      client.Client
	reconcileCount atomic.Int64
	cancelMgr      context.CancelFunc
)

type stubReconciler struct {
	client.Client
}

func (r *stubReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	reconcileCount.Add(1)
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
	Expect(runtimev1alpha1.AddToScheme(scheme)).To(Succeed())

	k8sClient, err = client.New(cfg, client.Options{Scheme: scheme})
	Expect(err).NotTo(HaveOccurred())

	mgr, err := ctrl.NewManager(cfg, ctrl.Options{Scheme: scheme})
	Expect(err).NotTo(HaveOccurred())

	Expect(
		ctrl.NewControllerManagedBy(mgr).For(&runtimev1alpha1.Artifact{}).Complete(&stubReconciler{Client: mgr.GetClient()}),
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

var _ = Describe("controller manager", func() {
	It("reconciles when an Artifact is created", func() {
		before := reconcileCount.Load()

		ctx := context.Background()
		a := &runtimev1alpha1.Artifact{
			ObjectMeta: metav1.ObjectMeta{Name: "reconcile-me", Namespace: "default"},
			Spec:       runtimev1alpha1.ArtifactSpec{Image: "ghcr.io/example/comp:v1"},
		}
		Expect(k8sClient.Create(ctx, a)).To(Succeed())

		Eventually(func() int64 {
			return reconcileCount.Load()
		}, 5*time.Second, 100*time.Millisecond).Should(BeNumerically(">", before))
	})
})
