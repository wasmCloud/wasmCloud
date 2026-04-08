package v1alpha1

import (
	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

const (
	WorkloadConditionHostSelection condition.ConditionType = "HostSelection"
	WorkloadConditionPlacement     condition.ConditionType = "Placement"
	WorkloadConditionConfig        condition.ConditionType = "Config"
	WorkloadConditionSync          condition.ConditionType = "Sync"
)

// EphemeralVolume represents a temporary directory that shares a workload's lifetime.
type EphemeralVolume struct{}

// HostPathVolume represents a pre-existing file or directory on the host machine.
type HostPathVolume struct {
	// Path of the file or directory on the host.
	// +kubebuilder:validation:Required
	Path string `json:"path"`
}

// Volume represents a named volume that can be mounted by components.
type Volume struct {
	// Name of the volume. Must be a DNS_LABEL and unique within the Workload.
	// +kubebuilder:validation:Required
	Name string `json:"name"`
	// EphemeralVolume represents a temporary directory that shares a workload's lifetime.
	// +kubebuilder:validation:Optional
	EphemeralVolume *EphemeralVolume `json:"ephemeral,omitempty"`
	// HostPathVolume represents a pre-existing file or directory on the host machine.
	// +kubebuilder:validation:Optional
	HostPathVolume *HostPathVolume `json:"hostPath,omitempty"`
}

// VolumeMount describes a mounting of a Volume within a component.
type VolumeMount struct {
	// Name must match the Name of a Volume defined in the WorkloadSpec.Volumes field.
	// +kubebuilder:validation:Required
	Name string `json:"name"`
	// MountPath is the path within the component where the volume should be mounted.
	// +kubebuilder:validation:Required
	MountPath string `json:"mountPath"`
	// ReadOnly indicates whether the volume should be mounted as read-only.
	// +kubebuilder:validation:Optional
	ReadOnly bool `json:"readOnly,omitempty"`
}

type ConfigLayer struct {
	// +kubebuilder:validation:Optional
	// ConfigFrom is a list of references to ConfigMaps that will be provided to the workload.
	// The keys and values of all referenced ConfigMaps will be merged. In case of key conflicts,
	// the last ConfigMap in the list wins.
	// +kubebuilder:validation:Optional
	ConfigFrom []corev1.LocalObjectReference `json:"configFrom,omitempty"`
	// The keys and values of all referenced Secrets will be merged. In case of key conflicts,
	// the last Secret in the list wins.
	// The values of the Secrets will be base64-decoded, utf-8 decoded before being provided to the workload.
	// +kubebuilder:validation:Optional
	SecretFrom []corev1.LocalObjectReference `json:"secretFrom,omitempty"`
	// +kubebuilder:validation:Optional
	Config map[string]string `json:"config,omitempty"`
}

// LocalResources describes resources that will be made available to a workload component.
type LocalResources struct {
	// VolumeMounts is a list of volume mounts that will be mounted into the workload component.
	// The volumes must be defined in the WorkloadSpec.Volumes field.
	// +kubebuilder:validation:Optional
	VolumeMounts []VolumeMount `json:"volumeMounts,omitempty"`
	// +kubebuilder:validation:Optional
	Environment *ConfigLayer `json:"environment,omitempty"`
	// +kubebuilder:validation:Optional
	Config map[string]string `json:"config,omitempty"`
	// +kubebuilder:validation:Optional
	AllowedHosts []string `json:"allowedHosts,omitempty"`
}

// WorkloadComponent represents a component of a workload.
// Components are stateless, invocation-driven units of computation.
// Components are isolated from each other and can be scaled independently.
// Each Component has a Root WIT World, representing the Components imports/exports. The combined
// list of all Components' Root WIT Worlds within a workload must be compatible with the Host's WIT World.
// All components within a workload are guaranteed to be placed on the same Wasm Host.
type WorkloadComponent struct {
	// +kubebuilder:validation:Required
	Name string `json:"name"`
	// +kubebuilder:validation:Required
	Image string `json:"image"`
	// +kubebuilder:validation:Optional
	ImagePullSecret *corev1.LocalObjectReference `json:"imagePullSecret,omitempty"`
	// +kubebuilder:validation:Optional
	ImagePullPolicy corev1.PullPolicy `json:"imagePullPolicy,omitempty"`
	// +kubebuilder:validation:Optional
	PoolSize int32 `json:"poolSize,omitempty"`
	// +kubebuilder:validation:Optional
	MaxInvocations int32 `json:"maxInvocations,omitempty"`
	// +kubebuilder:validation:Optional
	LocalResources *LocalResources `json:"localResources,omitempty"`
}

// WorkloadService represents a long-running service that is part of the workload.
// It is also sometimes referred to as a "sidecar" and is optional.
// A Service differs from a Component in that it is long-running and represents the Workload's "localhost".
// Services can bind to TCP & UDP ports, which are accessible by Components within the same workload via "localhost" or "127.0.0.1".
// Services export a single WIT interface, shaped as wasi:cli/run.
// Services can import interfaces from any Component within the same workload, or from the Host.
type WorkloadService struct {
	// +kubebuilder:validation:Required
	Image string `json:"image"`
	// +kubebuilder:validation:Optional
	ImagePullSecret *corev1.LocalObjectReference `json:"imagePullSecret,omitempty"`
	// +kubebuilder:validation:Optional
	ImagePullPolicy corev1.PullPolicy `json:"imagePullPolicy,omitempty"`
	// +kubebuilder:validation:Optional
	MaxRestarts int32 `json:"maxRestarts"`
	// +kubebuilder:validation:Optional
	LocalResources *LocalResources `json:"localResources,omitempty"`
}

type HostInterface struct {
	// Provides the config / configmap / secret references for this host interface
	ConfigLayer `json:",inline"`

	// Name uniquely identifies this interface instance when multiple entries
	// share the same namespace+package. Components use this name as the
	// identifier parameter in resource-opening functions (e.g., store::open(name)).
	// Required when multiple entries of the same namespace:package exist.
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:Pattern=`^[a-z0-9][a-z0-9-]*$`
	Name string `json:"name,omitempty"`

	// +kubebuilder:validation:Required
	Namespace string `json:"namespace"`
	// +kubebuilder:validation:Required
	Package string `json:"package"`
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:MinItems=1
	Interfaces []string `json:"interfaces,omitempty"`
	// +kubebuilder:validation:Optional
	Version string `json:"version,omitempty"`
}

func (h *HostInterface) HasInterface(iface string) bool {
	for _, existing := range h.Interfaces {
		if existing == iface {
			return true
		}
	}
	return false
}

func (h *HostInterface) EnsureInterfaces(ifaces ...string) {
	for _, iface := range ifaces {
		if !h.HasInterface(iface) {
			h.Interfaces = append(h.Interfaces, iface)
		}
	}
}

// KubernetesServiceRef references an existing Kubernetes Service that the
// operator will manage an EndpointSlice for, pointing to the host pods that
// are running this workload.
type KubernetesServiceRef struct {
	// Name is the name of the Kubernetes Service in the same namespace.
	// +kubebuilder:validation:Required
	Name string `json:"name"`
}

// KubernetesSpec groups Kubernetes-specific configuration for a workload.
type KubernetesSpec struct {
	// Service references an existing Kubernetes Service that the operator will
	// maintain an EndpointSlice for, pointing to the host pods running this
	// workload. When set, the operator also registers DNS aliases for the
	// service (e.g. service-name, service-name.namespace.svc.cluster.local)
	// with the host so cluster-internal callers can reach the workload via
	// Service DNS without going through an external gateway.
	// +kubebuilder:validation:Optional
	Service *KubernetesServiceRef `json:"service,omitempty"`
}

// WorkloadSpec defines the desired state of Workload.
type WorkloadSpec struct {
	// +kubebuilder:validation:Optional
	HostSelector map[string]string `json:"hostSelector,omitempty"`
	// +kubebuilder:validation:Optional
	HostID string `json:"hostId,omitempty"`

	// +kubebuilder:validation:Optional
	Components []WorkloadComponent `json:"components,omitempty"`
	// +kubebuilder:validation:Optional
	HostInterfaces []HostInterface `json:"hostInterfaces,omitempty"`
	// +kubebuilder:validation:Optional
	Service *WorkloadService `json:"service,omitempty"`
	// +kubebuilder:validation:Optional
	Volumes []Volume `json:"volumes,omitempty"`

	// Kubernetes groups Kubernetes-specific configuration such as Service
	// references and endpoint management.
	// +kubebuilder:validation:Optional
	Kubernetes *KubernetesSpec `json:"kubernetes,omitempty"`
}

func (s *WorkloadSpec) EnsureHostInterface(iface HostInterface) {
	for i, existing := range s.HostInterfaces {
		if existing.Namespace == iface.Namespace && existing.Package == iface.Package && existing.Name == iface.Name {
			existing.EnsureInterfaces(iface.Interfaces...)
			if iface.Config != nil && existing.Config == nil {
				existing.Config = make(map[string]string)
			}

			for k, v := range iface.Config {
				existing.Config[k] = v
			}
			s.HostInterfaces[i] = existing

			return
		}
	}
	s.HostInterfaces = append(s.HostInterfaces, iface)
}

// WorkloadStatus defines the observed state of Workload.
type WorkloadStatus struct {
	condition.ConditionedStatus `json:",inline"`
	// +kubebuilder:validation:Optional
	HostID string `json:"hostId,omitempty"`
	// +kubebuilder:validation:Optional
	WorkloadID string `json:"workloadId,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:shortName=ww
// +kubebuilder:printcolumn:name="HOSTID",type=string,JSONPath=".status.hostId"
// +kubebuilder:printcolumn:name="READY",type=string,JSONPath=`.status.conditions[?(@.type=="Ready")].status`
// +kubebuilder:printcolumn:name="AGE",type="date",JSONPath=".metadata.creationTimestamp"

// Workload is the Schema for the artifacts API.
type Workload struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   WorkloadSpec   `json:"spec,omitempty"`
	Status WorkloadStatus `json:"status,omitempty"`
}

// fulfill the ConditionedStatus interface
func (a *Workload) ConditionedStatus() *condition.ConditionedStatus {
	return &a.Status.ConditionedStatus
}

func (a *Workload) InitializeConditionedStatus() {
}

// +kubebuilder:object:root=true

// WorkloadList contains a list of Workload.
type WorkloadList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Workload `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Workload{}, &WorkloadList{})
}
