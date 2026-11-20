package runtime

import (
	"context"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
)

// These controller-only tests have no Workload reconciler. Complete its
// finalization after successful deletes so rollout assertions observe cleanup.
type finalizingTestClient struct{ client.WithWatch }

func (c finalizingTestClient) Delete(ctx context.Context, obj client.Object, opts ...client.DeleteOption) error {
	if err := c.WithWatch.Delete(ctx, obj, opts...); err != nil {
		return err
	}
	if _, ok := obj.(*runtimev1alpha1.Workload); !ok {
		return nil
	}
	current := &runtimev1alpha1.Workload{}
	if err := c.Get(ctx, client.ObjectKeyFromObject(obj), current); err != nil {
		return client.IgnoreNotFound(err)
	}
	base := current.DeepCopy()
	controllerutil.RemoveFinalizer(current, workloadFinalizerName)
	return c.Patch(ctx, current, client.MergeFrom(base))
}
