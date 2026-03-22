package v1alpha1

import (
	"fmt"
	"hash/fnv"

	"go.wasmcloud.dev/runtime-operator/api/condition"
	"k8s.io/apimachinery/pkg/util/rand"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/util/json"
)

const (
	WorkloadReplicaSetConditionScaleUp   condition.ConditionType = "ScaleUp"
	WorkloadReplicaSetConditionScaleDown condition.ConditionType = "ScaleDown"
)

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
	Replicas *int32 `json:"replicas,omitempty"`
	// +kubebuilder:validation:Required
	Template WorkloadReplicaTemplate `json:"template,omitempty"`
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

// WorkloadReplicaSet is the Schema for the artifacts API.
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
