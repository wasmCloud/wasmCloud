package runtime

import (
	"context"
	"fmt"
	"strings"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/labels"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// The /scale subresource needs a flat current count and a selector that
// matches the Workloads the ReplicaSet creates.
func TestReplicaSet_ScaleSubresourceStatus(t *testing.T) {
	// A 63-character WorkloadDeployment name is valid, but the RS made from
	// it is longer than a label value may be.
	longRS := strings.Repeat("d", 63) + "-" + randHash()
	for _, name := range []string{testReplicaSetName, longRS} {
		t.Run(fmt.Sprintf("name length %d", len(name)), func(t *testing.T) {
			testReplicaSetScaleStatus(t, name)
		})
	}
}

func testReplicaSetScaleStatus(t *testing.T, replicaSetName string) {
	s := newScaleTestScheme(t)
	replicaSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadReplicaSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}

	replicas := int32(2)
	rs := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{Namespace: testNamespace, Name: replicaSetName, UID: "rs-uid"},
		Spec: runtimev1alpha1.WorkloadReplicaSetSpec{
			Replicas: &replicas,
			Template: runtimev1alpha1.WorkloadReplicaTemplate{
				Labels: map[string]string{"app": "echo-rs"},
			},
		},
	}
	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(rs).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetNameIndex, func(obj client.Object) []string {
			if name, ok := isOwnedByController(obj, replicaSetGvk); ok {
				return []string{name}
			}
			return nil
		}).
		WithIndex(&runtimev1alpha1.Workload{}, workloadReplicaSetHashIndex, func(obj client.Object) []string {
			return []string{obj.GetAnnotations()[workloadReplicaSetGenerationAnnotation]}
		}).
		Build()
	r := &WorkloadReplicaSetReconciler{Client: c, Scheme: s}
	ctx := context.Background()

	if err := r.reconcileScaleUp(ctx, rs); err != nil {
		t.Fatalf("reconcileScaleUp: %v", err)
	}
	rs.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleUp),
		condition.ReadyCondition(runtimev1alpha1.WorkloadReplicaSetConditionScaleDown),
	)
	// The Workloads are not available yet, so Ready reports an error, but the
	// status fields are still filled in.
	_ = r.reconcileReady(ctx, rs)

	if rs.Status.CurrentReplicas != replicas {
		t.Fatalf("expected CurrentReplicas=%d, got %d", replicas, rs.Status.CurrentReplicas)
	}
	// labels.Parse rejects values the API server would.
	selector, err := labels.Parse(rs.Status.Selector)
	if err != nil {
		t.Fatalf("invalid Selector %q: %v", rs.Status.Selector, err)
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := c.List(ctx, workloads, client.MatchingLabelsSelector{Selector: selector}); err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(workloads.Items) != int(replicas) {
		t.Fatalf("expected the selector to match %d workloads, got %d", replicas, len(workloads.Items))
	}
	if workloads.Items[0].Labels["app"] != "echo-rs" {
		t.Fatalf("expected template labels kept, got %v", workloads.Items[0].Labels)
	}
	if rs.Spec.Template.Labels[workloadReplicaSetNameLabel] != "" {
		t.Fatalf("template labels were mutated: %v", rs.Spec.Template.Labels)
	}
}
