package v1alpha1

import (
	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

const ArtifactConditionSync condition.ConditionType = "Sync"
const ArtifactConditionPublished condition.ConditionType = "Published"
const ArtifactConditionPrecompiled condition.ConditionType = "Precompiled"
const ArtifactConditionPrecompileFailed condition.ConditionType = "PrecompileFailed"

// ArtifactSpec defines the desired state of Artifact.
type ArtifactSpec struct {
	// +kubebuilder:validation:Required
	Image string `json:"image"`
	// +kubebuilder:validation:Optional
	ImagePullSecret *corev1.LocalObjectReference `json:"imagePullSecret,omitempty"`
}

// PrecompiledVariant describes one precompiled output of an Artifact
// keyed by te (target, wasmtime-version) pair that produced it
type PrecompiledVariant struct {
	Target          string `json:"target"`
	WasmtimeVersion string `json:"wasmtimeVersion"`
	ArtifactURL     string `json:"artifactUrl"`
}

// ArtifactStatus defines the observed state of Artifact.
type ArtifactStatus struct {
	condition.ConditionedStatus `json:",inline"`
	// +kubebuilder:validation:Optional
	ObservedGeneration int64 `json:"observedGeneration,omitempty"`
	// +kubebuilder:validation:Optional
	ArtifactURL string `json:"artifactUrl,omitempty"`
	// +kubebuilder:validatoin:Optional
	Precompiled []PrecompiledVariant `json:"precompiled,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status

// Artifact is the Schema for the artifacts API.
type Artifact struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   ArtifactSpec   `json:"spec,omitempty"`
	Status ArtifactStatus `json:"status,omitempty"`
}

// fulfill the ConditionedStatus interface
func (a *Artifact) ConditionedStatus() *condition.ConditionedStatus {
	return &a.Status.ConditionedStatus
}

func (a *Artifact) InitializeConditionedStatus() {
}

// +kubebuilder:object:root=true

// ArtifactList contains a list of Artifact.
type ArtifactList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Artifact `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Artifact{}, &ArtifactList{})
}
