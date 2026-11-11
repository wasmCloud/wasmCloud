package runtime

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"sync/atomic"
	"testing"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/util/wait"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/config"
	"sigs.k8s.io/controller-runtime/pkg/metrics/server"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

type recoveryHeartbeatBus struct {
	mockBus
	heartbeat func(context.Context, *wasmbus.Message) (*wasmbus.Message, error)
}

func (b *recoveryHeartbeatBus) Request(ctx context.Context, msg *wasmbus.Message) (*wasmbus.Message, error) {
	if b.heartbeat != nil {
		return b.heartbeat(ctx, msg)
	}
	return &wasmbus.Message{Data: []byte("{}")}, nil
}

func (*recoveryHeartbeatBus) Subscribe(string, int) (wasmbus.Subscription, error) {
	return recoveryHeartbeatSubscription{}, nil
}

type recoveryHeartbeatSubscription struct{}

func (recoveryHeartbeatSubscription) Handle(wasmbus.SubscriptionCallback) {}
func (recoveryHeartbeatSubscription) Drain() error                        { return nil }

func TestReplacementHostBecomesReadyDuringCleanup(t *testing.T) {
	apiClient, mgr := newHostRecoveryManager(t)

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
		Scheme:             apiClient.Scheme(),
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

func newHostRecoveryManager(t *testing.T) (client.Client, ctrl.Manager) {
	t.Helper()
	apiClient, cfg := startHostEnvtestClient(t)
	mgr, err := ctrl.NewManager(cfg, ctrl.Options{
		Scheme:  apiClient.Scheme(),
		Metrics: server.Options{BindAddress: "0"},
		// The same controller can be started more than once with go test -count.
		Controller: config.Controller{SkipNameValidation: new(true)},
	})
	if err != nil {
		t.Fatal(err)
	}

	return apiClient, mgr
}

func TestReplacementHostReadyWithCleanupQueueSaturated(t *testing.T) {
	for _, beforeStartup := range []bool{true, false} {
		t.Run(fmt.Sprintf("deleting-before-startup=%t", beforeStartup), func(t *testing.T) {
			testSaturatedHostCleanup(t, beforeStartup)
		})
	}
}

func testSaturatedHostCleanup(t *testing.T, beforeStartup bool) {
	t.Helper()
	apiClient, mgr := newHostRecoveryManager(t)
	ctx, cancel := context.WithTimeout(t.Context(), 30*time.Second)
	defer cancel()
	ns := createTestNamespace(t, ctx, apiClient, "saturated-host-cleanup")
	// More departing Hosts than cleanup workers also leaves cleanup queued.
	hosts := make([]*runtimev1alpha1.Host, hostCleanupConcurrency+1)
	for i := range hosts {
		hosts[i] = createDepartingHost(t, ctx, apiClient, ns, fmt.Sprintf("departing-%d", i))
	}
	deleteHosts := func() {
		for _, host := range hosts {
			if err := apiClient.Delete(ctx, host); err != nil {
				t.Fatal(err)
			}
		}
	}
	if beforeStartup {
		deleteHosts()
	}

	started := make(chan struct{}, len(hosts))
	release := make(chan struct{})
	r := &HostReconciler{
		Client: &deleteTestClient{Client: mgr.GetClient(), delete: func(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
			if _, ok := obj.(*runtimev1alpha1.Workload); ok {
				started <- struct{}{}
				select {
				case <-release:
				case <-ctx.Done():
					return ctx.Err()
				}
			}
			return mgr.GetClient().Delete(ctx, obj, opts...)
		}},
		Scheme: apiClient.Scheme(), Bus: &recoveryHeartbeatBus{}, UnreachableTimeout: time.Minute,
	}
	if err := r.SetupWithManager(mgr); err != nil {
		t.Fatal(err)
	}
	managerDone := startHostRecoveryManager(t, ctx, cancel, mgr)
	if !mgr.GetCache().WaitForCacheSync(ctx) {
		t.Fatal("manager cache did not sync")
	}
	if !beforeStartup {
		for _, host := range hosts {
			waitForReplacementHostReady(t, ctx, apiClient, client.ObjectKeyFromObject(host))
		}
		deleteHosts()
	}
	for range hostCleanupConcurrency {
		select {
		case <-started:
		case <-managerDone:
			t.Fatal("manager stopped before cleanup workers were occupied")
		case <-ctx.Done():
			t.Fatal("cleanup workers did not start")
		}
	}

	replacement := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "replacement", Namespace: ns}, HostID: "replacement-id",
	}
	if err := apiClient.Create(ctx, replacement); err != nil {
		t.Fatal(err)
	}
	waitForReplacementHostReady(t, ctx, apiClient, client.ObjectKeyFromObject(replacement))
	close(release)
	if err := wait.PollUntilContextCancel(ctx, 25*time.Millisecond, true, func(ctx context.Context) (bool, error) {
		for _, host := range hosts {
			err := apiClient.Get(ctx, client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{})
			if !apierrors.IsNotFound(err) {
				return false, err
			}
		}
		var workloads runtimev1alpha1.WorkloadList
		err := apiClient.List(ctx, &workloads, client.InNamespace(ns))
		return len(workloads.Items) == 0, err
	}); err != nil {
		t.Fatalf("departing Hosts and their Workloads did not finish cleanup: %v", err)
	}
}

func createDepartingHost(t *testing.T, ctx context.Context, c client.Client, namespace, name string) *runtimev1alpha1.Host {
	t.Helper()
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: namespace, Finalizers: []string{hostFinalizerName}},
		HostID:     name + "-id",
	}
	if err := c.Create(ctx, host); err != nil {
		t.Fatal(err)
	}
	workload := workloadForHost(name, host.HostID)
	workload.Namespace = namespace
	if err := c.Create(ctx, workload); err != nil {
		t.Fatal(err)
	}
	workload.Status.HostID = host.HostID
	if err := c.Status().Update(ctx, workload); err != nil {
		t.Fatal(err)
	}
	return host
}
