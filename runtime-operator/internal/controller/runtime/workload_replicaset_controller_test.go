package runtime

import (
	"context"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// A replica set that would hand each replica its own messaging consumer group
// must fail before any Workload is minted, rather than silently duplicating
// every message across the replicas.
func TestReconcileScaleUp_MessagingWithoutConsumerGroup_MintsNoWorkloads(t *testing.T) {
	scheme := newScaleTestScheme(t)
	replicas := int32(2)
	replicaSet := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testNamespace,
			Name:      testReplicaSetName,
		},
		Spec: runtimev1alpha1.WorkloadReplicaSetSpec{
			Replicas: &replicas,
			Template: runtimev1alpha1.WorkloadReplicaTemplate{
				Spec: runtimev1alpha1.WorkloadSpec{
					HostInterfaces: []runtimev1alpha1.HostInterface{{
						Namespace:  "wasmcloud",
						Package:    "messaging",
						Interfaces: []string{"handler"},
						ConfigLayer: runtimev1alpha1.ConfigLayer{
							Config: map[string]string{"subscriptions": "tasks.>"},
						},
					}},
					Components: []runtimev1alpha1.WorkloadComponent{{
						Name:  "worker",
						Image: "example.com/worker:latest",
					}},
				},
			},
		},
	}

	fakeClient := fake.NewClientBuilder().WithScheme(scheme).WithObjects(replicaSet).Build()
	r := &WorkloadReplicaSetReconciler{Client: fakeClient, Scheme: scheme}

	if err := r.reconcileScaleUp(context.Background(), replicaSet); err == nil {
		t.Fatal("expected reconcileScaleUp to reject replicas > 1 without a consumer group")
	}

	workloads := &runtimev1alpha1.WorkloadList{}
	if err := fakeClient.List(context.Background(), workloads, client.InNamespace(testNamespace)); err != nil {
		t.Fatalf("list workloads: %v", err)
	}
	if len(workloads.Items) != 0 {
		t.Fatalf("expected no workloads, got %d", len(workloads.Items))
	}
}
