package v1alpha1

import (
	"go.wasmcloud.dev/runtime-operator/api/condition"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

const (
	HostConditionReporting condition.ConditionType = "Reporting"
)

// HostStatus defines the observed state of Host.
type HostStatus struct {
	condition.ConditionedStatus `json:",inline"`

	Version           string `json:"version"`
	OSName            string `json:"osName"`
	OSArch            string `json:"osArch"`
	OSKernel          string `json:"osKernel"`
	SystemCPUUsage    string `json:"systemCPUUsage"`
	SystemMemoryTotal int64  `json:"systemMemoryTotal"`
	SystemMemoryFree  int64  `json:"systemMemoryFree"`
	ComponentCount    int    `json:"componentCount,omitempty"`
	WorkloadCount     int    `json:"workloadCount,omitempty"`

	LastSeen metav1.Time `json:"lastSeen,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Cluster
// +kubebuilder:printcolumn:name="HOSTID",type=string,JSONPath=`.hostId`
// +kubebuilder:printcolumn:name="HOSTGROUP",type=string,JSONPath=`.metadata.labels.hostgroup`
// +kubebuilder:printcolumn:name="READY",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`
// +kubebuilder:printcolumn:name="AGE",type="date",JSONPath=".metadata.creationTimestamp"

// Host is the Schema for the Hosts API.
type Host struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	// +kubebuilder:validation:Required
	HostID string `json:"hostId"`
	// +kubebuilder:validation:Optional
	Hostname string `json:"hostname"`
	// +kubebuilder:validation:Optional
	HTTPPort uint32 `json:"httpPort"`

	Status HostStatus `json:"status,omitempty"`
}

// fulfill the ConditionedStatus interface
func (a *Host) ConditionedStatus() *condition.ConditionedStatus {
	return &a.Status.ConditionedStatus
}

func (a *Host) InitializeConditionedStatus() {
}

// +kubebuilder:object:root=true

// HostList contains a list of Host.
type HostList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Host `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Host{}, &HostList{})
}
