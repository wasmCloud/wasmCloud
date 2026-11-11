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
					return apierrors.NewConflict(schema.GroupResource{Group: runtimev1alpha1.GroupVersion.Group, Resource: "workloads"}, obj.GetName(), errors.New("version changed"))
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
