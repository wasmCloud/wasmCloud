package runtime

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"sync/atomic"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/util/wait"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/config"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	"sigs.k8s.io/controller-runtime/pkg/metrics/server"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

type recoveryHeartbeatBus struct{ mockBus }

func (*recoveryHeartbeatBus) Request(context.Context, *wasmbus.Message) (*wasmbus.Message, error) {
	return &wasmbus.Message{Data: []byte("{}")}, nil
}

func (*recoveryHeartbeatBus) Subscribe(string, int) (wasmbus.Subscription, error) {
	return recoveryHeartbeatSubscription{}, nil
}

type recoveryHeartbeatSubscription struct{}

func (recoveryHeartbeatSubscription) Handle(wasmbus.SubscriptionCallback) {}
func (recoveryHeartbeatSubscription) Drain() error                        { return nil }

func TestReplacementHostBecomesReadyDuringCleanup(t *testing.T) {
	if os.Getenv("KUBEBUILDER_ASSETS") == "" {
		t.Skip("KUBEBUILDER_ASSETS not set; run via make test for envtest")
	}
	env := &envtest.Environment{
		CRDDirectoryPaths:     []string{filepath.Join("..", "..", "..", "config", "crd", "bases")},
		ErrorIfCRDPathMissing: true,
	}
	cfg, err := env.Start()
	if err != nil {
		t.Fatalf("start envtest: %v", err)
	}
	t.Cleanup(func() {
		if err := env.Stop(); err != nil {
			t.Errorf("stop envtest: %v", err)
		}
	})

	scheme := runtime.NewScheme()
	if err := corev1.AddToScheme(scheme); err != nil {
		t.Fatal(err)
	}
	if err := runtimev1alpha1.AddToScheme(scheme); err != nil {
		t.Fatal(err)
	}
	apiClient, err := client.New(cfg, client.Options{Scheme: scheme})
	if err != nil {
		t.Fatal(err)
	}
	mgr, err := ctrl.NewManager(cfg, ctrl.Options{
		Scheme:  scheme,
		Metrics: server.Options{BindAddress: "0"},
		// The same controller can be started more than once with go test -count.
		Controller: config.Controller{SkipNameValidation: new(true)},
	})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	ns := createTestNamespace(t, ctx, apiClient, "host-recovery")
	old := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "departing-host", Namespace: ns, Finalizers: []string{hostFinalizerName}},
		HostID:     "old-host",
	}
	if err := apiClient.Create(ctx, old); err != nil {
		t.Fatalf("create predecessor: %v", err)
	}
	workload := workloadForHost("assigned", old.HostID)
	workload.Namespace = ns
	if err := apiClient.Create(ctx, workload); err != nil {
		t.Fatalf("create assigned Workload: %v", err)
	}
	workload.Status.HostID = old.HostID
	if err := apiClient.Status().Update(ctx, workload); err != nil {
		t.Fatalf("assign Workload: %v", err)
	}
	if err := apiClient.Delete(ctx, old); err != nil {
		t.Fatalf("delete predecessor: %v", err)
	}

	deleteStarted := make(chan struct{})
	var deleteAttempts atomic.Int32
	retryStarted := make(chan struct{})
	releaseRetry := make(chan struct{})
	releaseDelete := make(chan struct{})
	reconciler := &HostReconciler{
		Client: &deleteTestClient{
			Client: mgr.GetClient(),
			delete: func(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
				if obj.GetUID() == workload.UID {
					switch deleteAttempts.Add(1) {
					case 1:
						close(deleteStarted)
						select {
						case <-releaseDelete:
							return errors.New("transient workload deletion failure")
						case <-ctx.Done():
							return ctx.Err()
						}
					case 2:
						close(retryStarted)
						select {
						case <-releaseRetry:
						case <-ctx.Done():
							return ctx.Err()
						}
					}
				}
				return mgr.GetClient().Delete(ctx, obj, opts...)
			},
		},
		Scheme:             scheme,
		Bus:                &recoveryHeartbeatBus{},
		UnreachableTimeout: time.Minute,
	}
	if err := reconciler.SetupWithManager(mgr); err != nil {
		t.Fatal(err)
	}
	managerDone := startHostRecoveryManager(t, ctx, cancel, mgr)

	select {
	case <-deleteStarted:
	case <-managerDone:
		t.Fatal("manager stopped before predecessor cleanup started")
	case <-ctx.Done():
		t.Fatal("predecessor cleanup did not start")
	}

	replacement := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "replacement", Namespace: ns},
		HostID:     "new-host",
	}
	if err := apiClient.Create(ctx, replacement); err != nil {
		t.Fatalf("create replacement: %v", err)
	}
	waitForReplacementHostReady(t, ctx, apiClient, client.ObjectKeyFromObject(replacement))

	close(releaseDelete)
	select {
	case <-retryStarted:
	case <-ctx.Done():
		t.Fatal("predecessor cleanup was not retried after the delete failed")
	}
	if err := apiClient.Get(ctx, client.ObjectKeyFromObject(old), old); err != nil {
		t.Fatalf("get predecessor during retry: %v", err)
	}
	if !slices.Contains(old.Finalizers, hostFinalizerName) {
		t.Fatal("Host finalizer was removed before cleanup succeeded")
	}
	close(releaseRetry)
	if err := wait.PollUntilContextCancel(ctx, 25*time.Millisecond, true, func(ctx context.Context) (bool, error) {
		err := apiClient.Get(ctx, client.ObjectKeyFromObject(old), &runtimev1alpha1.Host{})
		return apierrors.IsNotFound(err), client.IgnoreNotFound(err)
	}); err != nil {
		t.Fatalf("predecessor did not finish cleanup: %v", err)
	}
}

func startHostRecoveryManager(t *testing.T, ctx context.Context, cancel context.CancelFunc, mgr ctrl.Manager) <-chan struct{} {
	t.Helper()
	managerDone := make(chan struct{})
	var managerErr error
	go func() {
		defer close(managerDone)
		managerErr = mgr.Start(ctx)
	}()
	t.Cleanup(func() {
		cancel()
		<-managerDone
		if managerErr != nil {
			t.Errorf("run manager: %v", managerErr)
		}
	})

	return managerDone
}

func waitForReplacementHostReady(t *testing.T, ctx context.Context, apiClient client.Client, key client.ObjectKey) {
	t.Helper()
	if err := wait.PollUntilContextCancel(ctx, 25*time.Millisecond, true, func(ctx context.Context) (bool, error) {
		current := &runtimev1alpha1.Host{}
		if err := apiClient.Get(ctx, key, current); err != nil {
			return false, err
		}
		return current.Status.AllTrue(condition.TypeReady), nil
	}); err != nil {
		t.Fatalf("replacement did not become Ready while predecessor cleanup was blocked: %v", err)
	}
}
