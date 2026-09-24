package v1alpha1

import (
	"errors"
	"testing"
)

func messagingReplicaSetSpec(replicas int32, interfaceConfig, componentConfig map[string]string) *WorkloadReplicaSetSpec {
	return &WorkloadReplicaSetSpec{
		Replicas: &replicas,
		Template: WorkloadReplicaTemplate{
			Spec: WorkloadSpec{
				HostInterfaces: []HostInterface{{
					Namespace:   "wasmcloud",
					Package:     "messaging",
					Interfaces:  []string{"handler"},
					ConfigLayer: ConfigLayer{Config: interfaceConfig},
				}},
				Components: []WorkloadComponent{{
					Name:           "worker",
					Image:          "example.com/worker:latest",
					LocalResources: &LocalResources{Config: componentConfig},
				}},
			},
		},
	}
}

func TestValidateMessagingConsumerGroups_MultipleReplicasWithoutConsumerGroup_Fails(t *testing.T) {
	spec := messagingReplicaSetSpec(2, map[string]string{"subscriptions": "tasks.>"}, nil)

	err := spec.ValidateMessagingConsumerGroups()
	if !errors.Is(err, errMissingMessagingConsumerGroup) {
		t.Fatalf("expected errMissingMessagingConsumerGroup, got %v", err)
	}
}

func TestValidateMessagingConsumerGroups_MultipleReplicasWithBroadcast_Succeeds(t *testing.T) {
	spec := messagingReplicaSetSpec(2, map[string]string{"subscriptions": "tasks.>"}, map[string]string{"consumer_group": "broadcast"})

	if err := spec.ValidateMessagingConsumerGroups(); err != nil {
		t.Fatalf("expected broadcast to opt in to per-replica delivery, got %v", err)
	}
}

func TestValidateMessagingConsumerGroups_MultipleReplicasWithSharedGroup_Succeeds(t *testing.T) {
	spec := messagingReplicaSetSpec(2, map[string]string{"subscriptions": "tasks.>", "consumer_group": "workers"}, nil)

	if err := spec.ValidateMessagingConsumerGroups(); err != nil {
		t.Fatalf("expected an interface-level consumer group to satisfy every component, got %v", err)
	}
}

func TestValidateMessagingConsumerGroups_SingleReplica_Succeeds(t *testing.T) {
	spec := messagingReplicaSetSpec(1, map[string]string{"subscriptions": "tasks.>"}, nil)

	if err := spec.ValidateMessagingConsumerGroups(); err != nil {
		t.Fatalf("expected a single replica to need no consumer group, got %v", err)
	}
}

func TestValidateMessagingConsumerGroups_WithoutMessagingInterface_Succeeds(t *testing.T) {
	replicas := int32(4)
	spec := &WorkloadReplicaSetSpec{
		Replicas: &replicas,
		Template: WorkloadReplicaTemplate{
			Spec: WorkloadSpec{
				HostInterfaces: []HostInterface{{
					Namespace:  "wasi",
					Package:    "http",
					Interfaces: []string{"incoming-handler"},
				}},
				Components: []WorkloadComponent{{Name: "api", Image: "example.com/api:latest"}},
			},
		},
	}

	if err := spec.ValidateMessagingConsumerGroups(); err != nil {
		t.Fatalf("expected a workload without messaging to be unaffected, got %v", err)
	}
}

func TestValidateMessagingConsumerGroups_ServiceWithoutConsumerGroup_Fails(t *testing.T) {
	replicas := int32(2)
	spec := &WorkloadReplicaSetSpec{
		Replicas: &replicas,
		Template: WorkloadReplicaTemplate{
			Spec: WorkloadSpec{
				HostInterfaces: []HostInterface{{
					Namespace:   "wasmcloud",
					Package:     "messaging",
					Interfaces:  []string{"handler"},
					ConfigLayer: ConfigLayer{Config: map[string]string{"subscriptions": "tasks.>"}},
				}},
				Service: &WorkloadService{Image: "example.com/service:latest"},
			},
		},
	}

	err := spec.ValidateMessagingConsumerGroups()
	if !errors.Is(err, errMissingMessagingConsumerGroup) {
		t.Fatalf("expected errMissingMessagingConsumerGroup, got %v", err)
	}
}
