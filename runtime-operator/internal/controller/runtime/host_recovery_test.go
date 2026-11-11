package runtime

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/client-go/util/retry"
	"sigs.k8s.io/controller-runtime/pkg/client"
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
	r := &HostReconciler{Client: &deleteTestClient{Client: base, delete: func(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
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
	for _, hostID := range []string{testHeartbeatHostID, "replacement-host"} {
		t.Run(hostID, func(t *testing.T) {
			original := workloadForHost("assigned", testHeartbeatHostID)
			original.UID, original.ResourceVersion = "same-object", "100"
			fresh := original.DeepCopy()
			fresh.Status.HostID, fresh.ResourceVersion = hostID, "101"
			base := newHostFinalizeClient(t, fresh)
			deletes := 0
			r := &HostReconciler{apiReader: base, Client: &deleteTestClient{Client: base, delete: func(_ context.Context, obj client.Object, opts ...client.DeleteOption) error {
				deletes++
				if deletes == 1 {
					return apierrors.NewConflict(runtimev1alpha1.GroupVersion.WithResource("workloads").GroupResource(), obj.GetName(), errors.New("version changed"))
				}
				if version := (&client.DeleteOptions{}).ApplyOptions(opts).Preconditions.ResourceVersion; *version != fresh.ResourceVersion {
					t.Errorf("delete used resourceVersion %s, want %s", *version, fresh.ResourceVersion)
				}
				return nil
			}}}
			if err := r.deleteAssignedWorkload(context.Background(), original, testHeartbeatHostID); err != nil {
				t.Fatal(err)
			}
			wantDeletes := 1
			if hostID == testHeartbeatHostID {
				wantDeletes = 2
			}
			if deletes != wantDeletes {
				t.Fatalf("delete called %d times, want %d", deletes, wantDeletes)
			}
		})
	}
}

// A conflict means the listed Workload is out of date, not that it may be
// deleted: whatever now lives under its name is left alone unless it is the
// same object, still assigned to the retiring host.
func TestRecoveryHostFinalizeLeavesUnconfirmedWorkloads(t *testing.T) {
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
			// The error is what keeps the Host finalizer for another pass.
			name: "never settles", live: []client.Object{touched},
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
