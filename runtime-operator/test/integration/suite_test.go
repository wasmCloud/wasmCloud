package integration

import (
	"context"
	"path/filepath"
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	"k8s.io/apimachinery/pkg/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/cache"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	metricsserver "sigs.k8s.io/controller-runtime/pkg/metrics/server"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimectrl "go.wasmcloud.dev/runtime-operator/v2/internal/controller/runtime"
)

var (
	testEnv       *envtest.Environment
	k8sClient     client.Client
	cancelMgr     context.CancelFunc
	testCache     client.Reader
	testHost      = newFakeHost()
	testCacheHold = newCacheHold()
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

	// Lets the finalize test hold a deleted Workload back from the cache (see cacheHold).
	mgr, err := ctrl.NewManager(cfg, ctrl.Options{
		Scheme:  scheme,
		Metrics: metricsserver.Options{BindAddress: "0"},
		Cache: cache.Options{ByObject: map[client.Object]cache.ByObject{
			&runtimev1alpha1.Workload{}: {Transform: testCacheHold.transform},
		}},
	})
	Expect(err).NotTo(HaveOccurred())

	// Runs the Workload controller against fakeHost; testCache lets the finalize
	// test see what the controller sees.
	Expect((&runtimectrl.WorkloadReconciler{
		Client: mgr.GetClient(),
		Scheme: mgr.GetScheme(),
		Bus:    testHost,
	}).SetupWithManager(mgr)).To(Succeed())
	testCache = mgr.GetCache()

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
