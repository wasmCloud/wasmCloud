package runtime

import (
	"context"
	"strings"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/util/validation"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestRolloutChildNameStaysValid(t *testing.T) {
	for _, parent := range []string{"short", strings.Repeat("x", 253), strings.Repeat("x", 241) + ".tail"} {
		got := rolloutChildName(parent, "owner-uid:template:0")
		if errors := validation.IsDNS1123Subdomain(got); len(errors) != 0 {
			t.Fatalf("child of %q is invalid: %v", parent, errors)
		}
		if got != rolloutChildName(parent, "owner-uid:template:0") {
			t.Fatal("child name changed across retries")
		}
		if got == rolloutChildName(parent, "another-owner:template:0") {
			t.Fatal("different owner reused child name")
		}
	}
}

func TestReplicaSetScaleUpWithLaggingCacheAndMissingSlot(t *testing.T) {
	scheme := newScaleTestScheme(t)
	replicas := int32(2)
	rs := baseReplicaSet(testReplicaSetName)
	rs.UID = testReplicaSetUID
	rs.Spec.Replicas = &replicas
	gvk, err := gvkForType(rs, scheme)
	if err != nil {
		t.Fatal(err)
	}
	server := fake.NewClientBuilder().WithScheme(scheme).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetOwnerIndex, replicaSetOwnerIndexValue(gvk)).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetHashIndex, replicaSetHashIndexValue(gvk)).
		Build()
	cached := interceptor.NewClient(server, interceptor.Funcs{
		List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
			if workloads, ok := list.(*runtimev1alpha1.WorkloadList); ok {
				workloads.Items = nil
				return nil
			}
			return c.List(ctx, list, opts...)
		},
	})
	r := &WorkloadReplicaSetReconciler{Client: cached, APIReader: server, Scheme: scheme}
	for range 3 {
		if err := r.reconcileScaleUp(t.Context(), rs); err != nil {
			t.Fatal(err)
		}
	}
	workloads := &runtimev1alpha1.WorkloadList{}
	if err := server.List(t.Context(), workloads); err != nil {
		t.Fatal(err)
	}
	if len(workloads.Items) != 2 {
		t.Fatalf("created %d replicas through stale cache, want two", len(workloads.Items))
	}
	if err := server.Delete(t.Context(), &workloads.Items[0]); err != nil {
		t.Fatal(err)
	}
	r.Client = server
	if err := r.reconcileScaleUp(t.Context(), rs); err != nil {
		t.Fatal(err)
	}
	if err := server.List(t.Context(), workloads); err != nil {
		t.Fatal(err)
	}
	if len(workloads.Items) != 2 {
		t.Fatalf("missing slot left %d replicas, want two", len(workloads.Items))
	}

	// A child with the expected name but another owner cannot be adopted.
	workload := &workloads.Items[0]
	workload.OwnerReferences = []metav1.OwnerReference{}
	if err := server.Update(t.Context(), workload); err != nil {
		t.Fatal(err)
	}
	r.Client = cached
	if err := r.reconcileScaleUp(t.Context(), rs); err == nil {
		t.Fatal("unowned workload was reused")
	}
}
