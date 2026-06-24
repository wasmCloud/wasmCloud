package v1alpha1

import (
	"fmt"

	"github.com/Masterminds/semver/v3"
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
	// AllowedHosts is the outbound egress allowlist for this component.
	//
	// Each entry must match one of:
	//   - "*"                       (allow all)
	//   - "host[:port]"             e.g. "example.com" or "example.com:8443"
	//   - "scheme://host[:port][/]" e.g. "https://api.example.com" or "https://api.example.com/"
	//   - "*.suffix[:port]"         e.g. "*.example.com" or "*.example.com:8443"
	//   - "scheme://*.suffix[:port][/]" e.g. "https://*.example.com"
	//
	// This is a hosts policy, not a URL policy: entries must not include a
	// path (beyond bare `/`), query string, or fragment. The wildcard must
	// be `*.<rest>` (leading dot required); a bare `*foo` is rejected.
	//
	// Empty or absent allowedHosts denies all outgoing requests
	// (fail-closed). To opt into unrestricted egress, set `allowedHosts:
	// ["*"]` explicitly. Final validation runs in the runtime. This regex
	// is an admission-time guard, not the source of truth.
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:items:Pattern=`^\*$|^([A-Za-z][A-Za-z0-9+.-]*://)(\*\.)?[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*(:[0-9]{1,5})?/?$|^(\*\.)?[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*(:[0-9]{1,5})?$`
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
	// share the same namespace+package. It is the `(implements <name>)` id the
	// host uses to route a component's named import to this interface's backend,
	// so two imports of the same namespace:package can resolve to different
	// backends.
	// Required when multiple entries of the same namespace:package exist.
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:Pattern=`^[a-z0-9][a-z0-9-]*$`
	// +kubebuilder:validation:MaxLength=64
	Name string `json:"name,omitempty"`

	// +kubebuilder:validation:Required
	// +kubebuilder:validation:MaxLength=128
	Namespace string `json:"namespace"`
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:MaxLength=128
	Package string `json:"package"`
	// +kubebuilder:validation:Required
	// +kubebuilder:validation:MinItems=1
	Interfaces []string `json:"interfaces,omitempty"`
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:MaxLength=64
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

	// Environment, if set, scopes scheduling to Hosts whose Environment
	// matches this value, regardless of the Workload's own namespace.
	// The value is matched against Host.Environment — typically a
	// Kubernetes namespace for in-cluster host pods, or any
	// operator-defined identifier for out-of-cluster hosts (e.g. a
	// region or data center). Only honored when the operator is started
	// with allowSharedHosts=true, or when Environment equals the
	// Workload's namespace.
	// +kubebuilder:validation:Optional
	Environment string `json:"environment,omitempty"`

	// +kubebuilder:validation:Optional
	Components []WorkloadComponent `json:"components,omitempty"`

	// HostInterfaces declares the host-provided interfaces this workload needs.
	// Two routing invariants are enforced at admission, complementing the
	// host-side checks:
	//   1. No two entries may be exact duplicates (same namespace, package,
	//      name, and version).
	//   2. At most one entry of a given namespace:package may be unnamed — the
	//      unnamed entry is the default route and cannot be shared. Declare
	//      distinct `name`s to route multiple imports of the same package to
	//      different backends. Semver-incompatible versions of the same package
	//      may coexist (they are distinct interfaces).
	// +kubebuilder:validation:Optional
	// +kubebuilder:validation:MaxItems=64
	// +kubebuilder:validation:XValidation:rule="self.all(x, self.exists_one(y, y.__namespace__ == x.__namespace__ && y.__package__ == x.__package__ && (has(y.name) ? y.name : '') == (has(x.name) ? x.name : '') && (has(y.version) ? y.version : '') == (has(x.version) ? x.version : '')))",message="hostInterfaces must not contain duplicate entries with the same namespace, package, name, and version"
	// +kubebuilder:validation:XValidation:rule="self.all(x, (has(x.name) && x.name != '') || self.filter(y, y.__namespace__ == x.__namespace__ && y.__package__ == x.__package__ && !(has(y.name) && y.name != '')).size() == 1)",message="at most one unnamed hostInterface is allowed per namespace:package; set a unique name to disambiguate multiple imports of the same package"
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
		// Merge only entries that share a routing identity (namespace, package,
		// name) AND a semver-compatible version. Following the component-model
		// canonical-version rules (and wit-parser's merge-imports-by-semver),
		// only compatible versions collapse: e.g. 0.2.1 and 0.2.6 (canonical
		// "0.2") merge keeping the higher, while 0.2 and 0.3 stay distinct.
		// Name is part of the identity, so differently-named interfaces of the
		// same package never merge.
		if existing.Namespace == iface.Namespace &&
			existing.Package == iface.Package &&
			existing.Name == iface.Name &&
			canonVersion(existing.Version) == canonVersion(iface.Version) {
			existing.EnsureInterfaces(iface.Interfaces...)
			if iface.Config != nil && existing.Config == nil {
				existing.Config = make(map[string]string)
			}

			for k, v := range iface.Config {
				existing.Config[k] = v
			}
			// Settle a compatible merge on the newer version.
			existing.Version = maxVersion(existing.Version, iface.Version)
			s.HostInterfaces[i] = existing

			return
		}
	}
	s.HostInterfaces = append(s.HostInterfaces, iface)
}

// canonVersion returns the canonical version prefix that determines whether two
// interface versions are compatible for deduplication, per the component-model
// `canonversion` rules:
//   - major > 0            -> "<major>"                 (1.2.3      -> "1")
//   - major == 0, minor>0  -> "<major>.<minor>"         (0.2.6-rc.1 -> "0.2")
//   - otherwise            -> "<major>.<minor>.<patch>" (0.0.1      -> "0.0.1")
//
// Compatible versions share a canonical prefix and so link by trivial string
// equality. An empty version canonicalizes to "" (unversioned); a version that
// does not parse as semver is returned verbatim, so it only matches an
// identical string rather than silently merging.
func canonVersion(v string) string {
	if v == "" {
		return ""
	}
	parsed, err := semver.NewVersion(v)
	if err != nil {
		return v
	}
	switch {
	case parsed.Major() > 0:
		return fmt.Sprintf("%d", parsed.Major())
	case parsed.Minor() > 0:
		return fmt.Sprintf("%d.%d", parsed.Major(), parsed.Minor())
	default:
		return fmt.Sprintf("%d.%d.%d", parsed.Major(), parsed.Minor(), parsed.Patch())
	}
}

// maxVersion returns the semver-greater of two compatible versions so a merge
// settles on the newer one. Unparseable versions sort below parseable ones; if
// both are unparseable the non-empty one (else a) is kept.
func maxVersion(a, b string) string {
	av, aerr := semver.NewVersion(a)
	bv, berr := semver.NewVersion(b)
	switch {
	case aerr != nil && berr != nil:
		if a == "" {
			return b
		}
		return a
	case aerr != nil:
		return b
	case berr != nil:
		return a
	default:
		if av.LessThan(bv) {
			return b
		}
		return a
	}
}

// WorkloadStatus defines the observed state of Workload.
type WorkloadStatus struct {
	condition.ConditionedStatus `json:",inline"`
	// +kubebuilder:validation:Optional
	HostID string `json:"hostId,omitempty"`
	// Environment records the Environment of the Host this Workload was
	// scheduled onto (Host.Environment). Populated by the scheduler at
	// host-selection time; reflects where the workload is actually
	// running, regardless of whether Spec.Environment was explicitly set.
	// +kubebuilder:validation:Optional
	Environment string `json:"environment,omitempty"`
	// +kubebuilder:validation:Optional
	WorkloadID string `json:"workloadId,omitempty"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:shortName=ww
// +kubebuilder:printcolumn:name="HOSTID",type=string,JSONPath=".status.hostId"
// +kubebuilder:printcolumn:name="ENVIRONMENT",type=string,JSONPath=".status.environment"
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
