package v1alpha1

import (
	"go.wasmcloud.dev/runtime-operator/api/condition"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

const (
	WorkloadDeploymentConditionArtifact condition.ConditionType = "Artifact"
	WorkloadDeploymentConditionSync     condition.ConditionType = "Sync"
	WorkloadDeploymentConditionDeploy   condition.ConditionType = "Deploy"
	WorkloadDeploymentConditionScale    condition.ConditionType = "Scale"
)

type WorkloadDeployPolicy string

const (
	WorkloadDeployPolicyRollingUpdate WorkloadDeployPolicy = "RollingUpdate"
	WorkloadDeployPolicyRecreate      WorkloadDeployPolicy = "Recreate"
)

type WorkloadDeploymentArtifact struct {
	// +kubebuilder:validation:Required
	Name string `json:"name"`
	// +kubebuilder:validation:Required
	ArtifactFrom corev1.LocalObjectReference `json:"artifactFrom"`
}

// WorkloadDeploymentSpec defines the desired state of WorkloadDeployment.
type WorkloadDeploymentSpec struct {
	WorkloadReplicaSetSpec `json:",inline"`

	// +kubebuilder:validation:Optional
	// +kubebuilder:default=RollingUpdate
	DeployPolicy WorkloadDeployPolicy `json:"deployPolicy,omitempty"`

	// +kubebuilder:validation:Optional
	Artifacts []WorkloadDeploymentArtifact `json:"artifacts,omitempty"`
}

// WorkloadDeploymentStatus defines the observed state of WorkloadDeployment.
type WorkloadDeploymentStatus struct {
	condition.ConditionedStatus `json:",inline"`
	// +kubebuilder:validation:Optional
	CurrentReplicaSet *corev1.LocalObjectReference `json:"currentReplicaSet,omitempty"`
	// +kubebuilder:validation:Optional
	PreviousReplicaSet *corev1.LocalObjectReference `json:"previousReplicaSet,omitempty"`
	// +kubebuilder:validation:Optional
	Replicas *ReplicaSetStatus `json:"replicas,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:printcolumn:name="REPLICAS",type=integer,JSONPath=`.spec.replicas`
// +kubebuilder:printcolumn:name="READY",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`

// WorkloadDeployment is the Schema for the artifacts API.
type WorkloadDeployment struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   WorkloadDeploymentSpec   `json:"spec,omitempty"`
	Status WorkloadDeploymentStatus `json:"status,omitempty"`
}

// fulfill the ConditionedStatus interface
func (a *WorkloadDeployment) ConditionedStatus() *condition.ConditionedStatus {
	return &a.Status.ConditionedStatus
}

func (a *WorkloadDeployment) InitializeConditionedStatus() {
}

// +kubebuilder:object:root=true

// WorkloadDeploymentList contains a list of HttpTrigger.
type WorkloadDeploymentList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []WorkloadDeployment `json:"items"`
}

func init() {
	SchemeBuilder.Register(&WorkloadDeployment{}, &WorkloadDeploymentList{})
}
