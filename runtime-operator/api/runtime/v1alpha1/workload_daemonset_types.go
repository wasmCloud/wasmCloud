package v1alpha1

import (
	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

const (
	WorkloadDaemonSetConditionArtifact condition.ConditionType = "Artifact"
	WorkloadDaemonSetConditionSchedule condition.ConditionType = "Schedule"
)

// WorkloadDaemonSetSpec defines the desired state of WorkloadDaemonSet.
type WorkloadDaemonSetSpec struct {
	// Replicas is the number of Workload instances to run on each Host that
	// matches template.spec.hostSelector.
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:Minimum=0
	// +kubebuilder:default=1
	Replicas *int32 `json:"replicas,omitempty"`

	// +kubebuilder:validation:Required
	Template WorkloadReplicaTemplate `json:"template,omitempty"`

	// DeployPolicy is how a template change reaches the hosts. RollingUpdate
	// starts the new instances on a host before stopping the old ones there,
	// MaxConcurrentHosts hosts at a time. Recreate stops every old instance at once.
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:Enum=RollingUpdate;Recreate
	// +kubebuilder:default=RollingUpdate
	DeployPolicy WorkloadDeployPolicy `json:"deployPolicy,omitempty"`

	// MaxConcurrentHosts is how many hosts a RollingUpdate updates at once.
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:Minimum=1
	// +kubebuilder:default=1
	MaxConcurrentHosts *int32 `json:"maxConcurrentHosts,omitempty"`

	// +kubebuilder:validation:Optional
	Artifacts []WorkloadDeploymentArtifact `json:"artifacts,omitempty"`

	// Kubernetes groups Kubernetes-specific configuration such as Service
	// references and endpoint management.
	// +kubebuilder:validation:Optional
	Kubernetes *KubernetesSpec `json:"kubernetes,omitempty"`
}

// WorkloadDaemonSetHostStatus is the observed state of one Host's instances.
type WorkloadDaemonSetHostStatus struct {
	// +kubebuilder:validation:Required
	HostID string `json:"hostId"`
	// Current is the number of instances of the current template on the host.
	// +kubebuilder:validation:Optional
	Current int32 `json:"current,omitempty"`
	// Ready is the number of available instances of the current template.
	// +kubebuilder:validation:Optional
	Ready int32 `json:"ready,omitempty"`
	// Outdated is the number of instances of a previous template still on the host.
	// +kubebuilder:validation:Optional
	Outdated int32 `json:"outdated,omitempty"`
}

// WorkloadDaemonSetStatus defines the observed state of WorkloadDaemonSet.
type WorkloadDaemonSetStatus struct {
	condition.ConditionedStatus `json:",inline"`

	// DesiredHosts is the number of schedulable Hosts that match the selector.
	// +kubebuilder:validation:Optional
	DesiredHosts int32 `json:"desiredHosts,omitempty"`
	// ReadyHosts is the number of desired Hosts running every instance of the
	// current template.
	// +kubebuilder:validation:Optional
	ReadyHosts int32 `json:"readyHosts,omitempty"`
	// UpdatedHosts is the number of desired Hosts with no instance of a
	// previous template left.
	// +kubebuilder:validation:Optional
	UpdatedHosts int32 `json:"updatedHosts,omitempty"`

	// Replicas totals the instances across Hosts; Expected is DesiredHosts × spec.replicas.
	// +kubebuilder:validation:Optional
	Replicas *ReplicaSetStatus `json:"replicas,omitempty"`

	// Hosts reports the instances on each Host, sorted by HostID.
	// +kubebuilder:validation:Optional
	Hosts []WorkloadDaemonSetHostStatus `json:"hosts,omitempty"`

	// CurrentReplicas is the average number of current-template instances
	// per desired Host, rounded down. It is the /scale statusReplicasPath, so
	// it counts per Host like spec.replicas does; replicas.current is the total.
	// +kubebuilder:validation:Optional
	CurrentReplicas int32 `json:"currentReplicas,omitempty"`

	// Selector is the serialized label selector exposed by the /scale subresource.
	// +kubebuilder:validation:Optional
	Selector string `json:"selector,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:subresource:scale:specpath=.spec.replicas,statuspath=.status.currentReplicas,selectorpath=.status.selector
// +kubebuilder:printcolumn:name="REPLICAS",type=integer,JSONPath=`.spec.replicas`
// +kubebuilder:printcolumn:name="DESIRED",type=integer,JSONPath=`.status.desiredHosts`
// +kubebuilder:printcolumn:name="UP-TO-DATE",type=integer,JSONPath=`.status.updatedHosts`
// +kubebuilder:printcolumn:name="READY-HOSTS",type=integer,JSONPath=`.status.readyHosts`
// +kubebuilder:printcolumn:name="READY",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`

// WorkloadDaemonSet runs a Workload on every Host that matches its host selector, spec.replicas instances per Host, and follows the Host set as it changes.
type WorkloadDaemonSet struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   WorkloadDaemonSetSpec   `json:"spec,omitempty"`
	Status WorkloadDaemonSetStatus `json:"status,omitempty"`
}

// fulfill the ConditionedStatus interface
func (a *WorkloadDaemonSet) ConditionedStatus() *condition.ConditionedStatus {
	return &a.Status.ConditionedStatus
}

func (a *WorkloadDaemonSet) InitializeConditionedStatus() {
}

// +kubebuilder:object:root=true

// WorkloadDaemonSetList contains a list of WorkloadDaemonSet.
type WorkloadDaemonSetList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []WorkloadDaemonSet `json:"items"`
}

func init() {
	SchemeBuilder.Register(&WorkloadDaemonSet{}, &WorkloadDaemonSetList{})
}
