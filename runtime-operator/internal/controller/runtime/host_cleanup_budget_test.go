package runtime

import (
	"context"
	"errors"
	"fmt"
	"sync/atomic"
	"testing"
	"testing/synctest"

	"golang.org/x/sync/semaphore"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/event"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const otherCleanupHostID = "other-cleanup-host"

func cleanupBudgetClient(t *testing.T) client.Client {
	t.Helper()
	objects := make([]client.Object, 0, 4*hostFinalizeDeleteConcurrency)
	for _, hostID := range []string{testHeartbeatHostID, otherCleanupHostID} {
		for i := range 2 * hostFinalizeDeleteConcurrency {
			objects = append(objects, workloadForHost(fmt.Sprintf("%s-%d", hostID, i), hostID))
		}
	}
	return newHostFinalizeClient(t, objects...)
}

func TestHostCleanupSharesDeleteBudget(t *testing.T) {
	synctest.Test(t, testHostCleanupSharesDeleteBudget)
}

func testHostCleanupSharesDeleteBudget(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	base := cleanupBudgetClient(t)
	release := make(chan struct{})
	var inFlight, attempts atomic.Int32
	r := &HostReconciler{apiReader: base, deleteSlots: semaphore.NewWeighted(hostFinalizeDeleteConcurrency)}
	r.Client = &deleteTestClient{Client: base, delete: func(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
		if n := inFlight.Add(1); n > hostFinalizeDeleteConcurrency {
			t.Errorf("combined delete concurrency = %d, limit = %d", n, hostFinalizeDeleteConcurrency)
		}
		defer inFlight.Add(-1)
		attempts.Add(1)
		select {
		case <-release:
			return base.Delete(ctx, obj, opts...)
		case <-ctx.Done():
			return ctx.Err()
		}
	}}
	done := make(chan error, 2)
	for _, hostID := range []string{testHeartbeatHostID, otherCleanupHostID} {
		go func() { done <- r.finalize(ctx, &runtimev1alpha1.Host{HostID: hostID}) }()
	}
	// Both finalizers have reached a blocking operation; no scheduling sleeps.
	synctest.Wait()
	if n := inFlight.Load(); n != hostFinalizeDeleteConcurrency {
		t.Fatalf("in-flight deletes = %d, want %d", n, hostFinalizeDeleteConcurrency)
	}
	close(release)
	for range 2 {
		if err := <-done; err != nil {
			t.Fatal(err)
		}
	}
	if n := attempts.Load(); n != 4*hostFinalizeDeleteConcurrency {
		t.Fatalf("delete attempts = %d, want all %d Workloads", n, 4*hostFinalizeDeleteConcurrency)
	}
	assertCleanupDrained(t, r, base)
}

func TestHostCleanupReleasesBudgetAfterFailure(t *testing.T) {
	for _, canceled := range []bool{false, true} {
		t.Run(fmt.Sprintf("canceled=%t", canceled), func(t *testing.T) {
			synctest.Test(t, func(t *testing.T) { testCleanupBudgetAfterFailure(t, canceled) })
		})
	}
}

func testCleanupBudgetAfterFailure(t *testing.T, canceled bool) {
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	firstCtx, cancelFirst := context.WithCancel(ctx)
	defer cancelFirst()
	base := cleanupBudgetClient(t)
	failure := errors.New("delete unavailable")
	release := make(chan struct{})
	block := true
	r := &HostReconciler{apiReader: base, deleteSlots: semaphore.NewWeighted(hostFinalizeDeleteConcurrency)}
	r.Client = &deleteTestClient{Client: base, delete: func(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
		if block && obj.(*runtimev1alpha1.Workload).Status.HostID == testHeartbeatHostID {
			select {
			case <-release:
				return failure
			case <-ctx.Done():
				return ctx.Err()
			}
		}
		return base.Delete(ctx, obj, opts...)
	}}
	firstDone, secondDone := make(chan error, 1), make(chan error, 1)
	firstHost := &runtimev1alpha1.Host{HostID: testHeartbeatHostID}
	go func() { firstDone <- r.finalize(firstCtx, firstHost) }()
	synctest.Wait()
	go func() { secondDone <- r.finalize(ctx, &runtimev1alpha1.Host{HostID: otherCleanupHostID}) }()
	synctest.Wait()
	wantErr := failure
	if canceled {
		wantErr = context.Canceled
		cancelFirst()
	} else {
		close(release)
	}
	if err := <-firstDone; !errors.Is(err, wantErr) {
		t.Fatalf("failed Host cleanup: %v, want %v", err, wantErr)
	}
	if err := <-secondDone; err != nil {
		t.Fatalf("one Host's failure prevented the other from completing: %v", err)
	}
	block = false
	if err := r.finalize(ctx, firstHost); err != nil {
		t.Fatalf("retry could not finish cleanup: %v", err)
	}
	assertCleanupDrained(t, r, base)
}

func assertCleanupDrained(t *testing.T, r *HostReconciler, c client.Client) {
	t.Helper()
	if !r.deleteSlots.TryAcquire(hostFinalizeDeleteConcurrency) {
		t.Fatal("cleanup leaked delete capacity")
	}
	r.deleteSlots.Release(hostFinalizeDeleteConcurrency)
	var workloads runtimev1alpha1.WorkloadList
	if err := c.List(t.Context(), &workloads); err != nil {
		t.Fatal(err)
	}
	if len(workloads.Items) != 0 {
		t.Fatalf("cleanup left %d Workloads", len(workloads.Items))
	}
}

func TestHostCleanupPredicateAdmitsDeletionOnce(t *testing.T) {
	live := &runtimev1alpha1.Host{}
	deleting := live.DeepCopy()
	now := metav1.Now()
	deleting.DeletionTimestamp = &now
	heartbeat := deleting.DeepCopy()
	heartbeat.Status.SystemCPUUsage = "1"

	p := hostCleanupPredicate()
	for _, tc := range []struct {
		name string
		got  bool
		want bool
	}{
		{"create live", p.Create(event.CreateEvent{Object: live}), false},
		{"create deleting (restart)", p.Create(event.CreateEvent{Object: deleting}), true},
		{"deletion starts", p.Update(event.UpdateEvent{ObjectOld: live, ObjectNew: deleting}), true},
		{"heartbeat while deleting", p.Update(event.UpdateEvent{ObjectOld: deleting, ObjectNew: heartbeat}), false},
		{"live update", p.Update(event.UpdateEvent{ObjectOld: live, ObjectNew: live}), false},
		{"delete", p.Delete(event.DeleteEvent{Object: deleting}), false},
	} {
		if tc.got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, tc.got, tc.want)
		}
	}
}
