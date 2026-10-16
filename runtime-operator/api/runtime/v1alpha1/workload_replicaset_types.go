package v1alpha1

import (
	"errors"
	"fmt"
	"hash/fnv"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	"k8s.io/apimachinery/pkg/util/rand"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/util/json"
)

const (
	WorkloadReplicaSetConditionScaleUp   condition.ConditionType = "ScaleUp"
	WorkloadReplicaSetConditionScaleDown condition.ConditionType = "ScaleDown"

	messagingInterfaceNamespace     = "wasmcloud"
	messagingInterfacePackage       = "messaging"
	messagingConsumerGroupConfigKey = "consumer_group"
	broadcastConsumerGroup          = "broadcast"
)

var errMissingMessagingConsumerGroup = errors.New("workload replicas > 1 without an explicit messaging consumer group")

type WorkloadReplicaTemplate struct {
	// +kubebuilder:validation:Optional
	Annotations map[string]string `json:"annotations,omitempty"`
	// +kubebuilder:validation:Optional
	Labels map[string]string `json:"labels,omitempty"`
	// +kubebuilder:validation:Required
	Spec WorkloadSpec `json:"spec,omitempty"`
}

func (w *WorkloadReplicaTemplate) Hash() string {
	h := fnv.New32a()
	rawSpec, _ := json.Marshal(w.Spec)
	_, _ = h.Write(rawSpec)
	return rand.SafeEncodeString(fmt.Sprint(h.Sum32()))
}

// WorkloadReplicaSetSpec defines the desired state of WorkloadReplicaSet.
type WorkloadReplicaSetSpec struct {
	// +kubebuilder:validation:Optional
	// +kubebuilder:default=1
	Replicas *int32 `json:"replicas,omitempty"`
	// +kubebuilder:validation:Required
	Template WorkloadReplicaTemplate `json:"template,omitempty"`
}

// ValidateMessagingConsumerGroups rejects a replica set that would give every
// replica a consumer group of its own. Each replica is a Workload carrying a
// distinct name, and the runtime derives its default consumer group from that
// name, so `replicas: 2` without an explicit `consumer_group` subscribes two
// groups and every message is delivered twice instead of load-balanced. Setting
// the key to `broadcast` is how a workload asks for that fan-out on purpose.
func (s *WorkloadReplicaSetSpec) ValidateMessagingConsumerGroups() error {
	if s.Replicas == nil || *s.Replicas <= 1 {
		return nil
	}

	missing := func(item string) error {
		return fmt.Errorf(
			"%w: set %q on %s to a shared group name so replicas load-balance, or to %q so every replica receives every message",
			errMissingMessagingConsumerGroup, messagingConsumerGroupConfigKey, item, broadcastConsumerGroup)
	}

	for _, iface := range s.Template.Spec.HostInterfaces {
		if iface.Namespace != messagingInterfaceNamespace || iface.Package != messagingInterfacePackage {
			continue
		}
		if hasConsumerGroup(iface.Config) {
			continue
		}
		for _, component := range s.Template.Spec.Components {
			if component.LocalResources == nil || !hasConsumerGroup(component.LocalResources.Config) {
				return missing(fmt.Sprintf("component %q", component.Name))
			}
		}
		if service := s.Template.Spec.Service; service != nil {
			if service.LocalResources == nil || !hasConsumerGroup(service.LocalResources.Config) {
				return missing("the workload service")
			}
		}
	}

	return nil
}

func hasConsumerGroup(config map[string]string) bool {
	_, ok := config[messagingConsumerGroupConfigKey]
	return ok
}

type ReplicaSetStatus struct {
	// +kubebuilder:validation:Optional
	Expected int32 `json:"expected,omitempty"`
	// +kubebuilder:validation:Optional
	Current int32 `json:"current,omitempty"`
	// +kubebuilder:validation:Optional
	Ready int32 `json:"ready,omitempty"`
	// +kubebuilder:validation:Optional
	Unavailable int32 `json:"unavailable,omitempty"`
}

// WorkloadReplicaSetStatus defines the observed state of WorkloadReplicaSet.
type WorkloadReplicaSetStatus struct {
	condition.ConditionedStatus `json:",inline"`
	// +kubebuilder:validation:Optional
	Replicas *ReplicaSetStatus `json:"replicas,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:printcolumn:name="REPLICAS",type=integer,JSONPath=`.spec.replicas`
// +kubebuilder:printcolumn:name="READY",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`

// WorkloadReplicaSet maintains a specified number of running Workload replicas; it is normally created and managed by a WorkloadDeployment.
type WorkloadReplicaSet struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   WorkloadReplicaSetSpec   `json:"spec,omitempty"`
	Status WorkloadReplicaSetStatus `json:"status,omitempty"`
}

// fulfill the ConditionedStatus interface
func (a *WorkloadReplicaSet) ConditionedStatus() *condition.ConditionedStatus {
	return &a.Status.ConditionedStatus
}

func (a *WorkloadReplicaSet) InitializeConditionedStatus() {
}

// +kubebuilder:object:root=true

// WorkloadReplicaSetList contains a list of WorkloadReplicaSet.
type WorkloadReplicaSetList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []WorkloadReplicaSet `json:"items"`
}

func init() {
	SchemeBuilder.Register(&WorkloadReplicaSet{}, &WorkloadReplicaSetList{})
}
