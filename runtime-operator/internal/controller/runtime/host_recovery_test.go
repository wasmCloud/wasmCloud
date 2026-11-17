package runtime

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	"golang.org/x/sync/semaphore"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/util/retry"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/config"
)

type deleteTestClient struct {
	client.Client
	delete func(context.Context, client.Object, ...client.DeleteOption) error
}

func (c *deleteTestClient) Delete(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
	return c.delete(ctx, obj, opts...)
}

func TestRecoveryHostFinalizeBoundsParallelDeletes(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	objs := make([]client.Object, 0, 2*hostFinalizeDeleteConcurrency)
	for i := range 2 * hostFinalizeDeleteConcurrency {
		objs = append(objs, workloadForHost(fmt.Sprintf("assigned-%d", i), testHeartbeatHostID))
	}
	started := make(chan struct{}, len(objs))
	release := make(chan struct{})
	deleteFailure := errors.New("delete failed")
	base := newHostFinalizeClient(t, objs...)
	r := &HostReconciler{deleteSlots: semaphore.NewWeighted(hostFinalizeDeleteConcurrency), Client: &deleteTestClient{Client: base, delete: func(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
		preconditions := (&client.DeleteOptions{}).ApplyOptions(opts).Preconditions
		if preconditions == nil || preconditions.UID == nil || preconditions.ResourceVersion == nil {
			return errors.New("delete lacked identity and version preconditions")
		}
		started <- struct{}{}
		select {
		case <-release:
			return deleteFailure
		case <-ctx.Done():
			return ctx.Err()
		}
	}}}

	done := make(chan error, 1)
	go func() { done <- r.finalize(ctx, &runtimev1alpha1.Host{HostID: testHeartbeatHostID}) }()
	for range hostFinalizeDeleteConcurrency {
		select {
		case <-started:
		case <-ctx.Done():
			t.Fatal("delete fan-out did not reach configured concurrency")
		}
	}
	select {
	case <-started:
		t.Fatal("more than the configured number of deletes were in flight")
	case <-time.After(25 * time.Millisecond):
	}
	close(release)
	if err := <-done; !errors.Is(err, deleteFailure) {
		t.Fatalf("finalize did not propagate the error needed to retain the Host finalizer: %v", err)
	}
}

func TestRecoveryHostFinalizeRefreshesConflict(t *testing.T) {
	const originalUID = "same-object"
	for _, tc := range []struct {
		name, hostID, uid string
		wantDeletes       int
	}{
		{"same assignment", testHeartbeatHostID, originalUID, 2},
		{"reassigned", "replacement-host", originalUID, 1},
		{"recreated", testHeartbeatHostID, "replacement-object", 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			original := workloadForHost("assigned", testHeartbeatHostID)
			original.UID, original.ResourceVersion = originalUID, "100"
			fresh := original.DeepCopy()
			fresh.Status.HostID, fresh.ResourceVersion, fresh.UID = tc.hostID, "101", types.UID(tc.uid)
			base := newHostFinalizeClient(t, fresh)
			deletes := 0
			r := &HostReconciler{apiReader: base, Client: &deleteTestClient{Client: base, delete: func(_ context.Context, obj client.Object, opts ...client.DeleteOption) error {
				deletes++
				if deletes == 1 {
					return apierrors.NewConflict(schema.GroupResource{}, obj.GetName(), errors.New("version changed"))
				}
				if version := (&client.DeleteOptions{}).ApplyOptions(opts).Preconditions.ResourceVersion; *version != fresh.ResourceVersion {
					t.Errorf("delete used resourceVersion %s, want %s", *version, fresh.ResourceVersion)
				}
				return nil
			}}}
			if err := r.deleteAssignedWorkload(context.Background(), original, testHeartbeatHostID); err != nil {
				t.Fatal(err)
			}
			if deletes != tc.wantDeletes {
				t.Fatalf("delete called %d times, want %d", deletes, tc.wantDeletes)
			}
		})
	}
}

// A conflict means the listed Workload is out of date, not that it may be
// deleted: whatever now lives under its name is deleted only if it is the same
// object, still assigned to the retiring host. If it keeps changing, the retries
// run out and the conflict is returned, which keeps the Host finalizer.
func TestRecoveryHostFinalizeRechecksConflictedWorkloads(t *testing.T) {
	original := workloadForHost("assigned", testHeartbeatHostID)
	original.UID, original.ResourceVersion = "same-object", "100"
	recreated := original.DeepCopy()
	recreated.UID, recreated.ResourceVersion = "new-object", "101"
	touched := original.DeepCopy()
	touched.ResourceVersion = "101"

	for _, tt := range []struct {
		name string
		// live is what the API server holds when the conflict is refreshed.
		live        []client.Object
		wantDeletes int
		wantErr     bool
	}{
		{name: "recreated under the same name", live: []client.Object{recreated}, wantDeletes: 1},
		{name: "already gone", wantDeletes: 1},
		{
			// Each retry rereads, finds the same object, and conflicts again.
			name: "keeps conflicting until retries run out", live: []client.Object{touched},
			wantDeletes: retry.DefaultRetry.Steps, wantErr: true,
		},
	} {
		t.Run(tt.name, func(t *testing.T) {
			base := newHostFinalizeClient(t, tt.live...)
			deletes := 0
			r := &HostReconciler{apiReader: base, Client: &deleteTestClient{Client: base, delete: func(_ context.Context, obj client.Object, _ ...client.DeleteOption) error {
				deletes++
				return apierrors.NewConflict(runtimev1alpha1.GroupVersion.WithResource("workloads").GroupResource(), obj.GetName(), errors.New("version changed"))
			}}}
			err := r.deleteAssignedWorkload(context.Background(), original, testHeartbeatHostID)
			if tt.wantErr != apierrors.IsConflict(err) || (!tt.wantErr && err != nil) {
				t.Fatalf("deleteAssignedWorkload returned %v, want conflict: %t", err, tt.wantErr)
			}
			if deletes != tt.wantDeletes {
				t.Fatalf("delete called %d times, want %d", deletes, tt.wantDeletes)
			}
		})
	}
}

func TestHostControllerKeepsAWorkerForReplacements(t *testing.T) {
	host := runtimev1alpha1.GroupVersion.WithKind("Host").GroupKind().String()
	for _, tt := range []struct {
		name    string
		manager config.Controller
		want    int
	}{
		{name: "untuned", want: minHostWorkers},
		{name: "manager-wide single worker", manager: config.Controller{MaxConcurrentReconciles: 1}, want: minHostWorkers},
		{name: "manager-wide more workers", manager: config.Controller{MaxConcurrentReconciles: 6}, want: 6},
		{name: "host single worker", manager: config.Controller{GroupKindConcurrency: map[string]int{host: 1}}, want: minHostWorkers},
		{
			name:    "host entry wins over manager-wide",
			manager: config.Controller{MaxConcurrentReconciles: 1, GroupKindConcurrency: map[string]int{host: 4}},
			want:    4,
		},
		{
			name:    "other kinds are ignored",
			manager: config.Controller{GroupKindConcurrency: map[string]int{"Workload.runtime.wasmcloud.dev": 9}},
			want:    minHostWorkers,
		},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := hostControllerOptions(tt.manager).MaxConcurrentReconciles; got != tt.want {
				t.Fatalf("got %d workers, want %d", got, tt.want)
			}
		})
	}
}
