package runtime

import (
	"context"
	"errors"
	"maps"
	"slices"
	"testing"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/rest"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/config"
	"sigs.k8s.io/controller-runtime/pkg/manager"
	metricsserver "sigs.k8s.io/controller-runtime/pkg/metrics/server"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

var errRegistration = errors.New("registration refused")

// Values the index extraction cases look for.
const (
	indexedHostID                  = "indexed-host"
	indexedDeploymentUID types.UID = "deployment-uid"
	indexedDaemonSetUID  types.UID = "daemonset-uid"
	indexedReplicaSetUID types.UID = "replicaset-uid"
)

// recordingManager records the field indexes a controller registers instead
// of building informers for them, so setup runs without an API server. It
// refuses the index named failIndex, and every runnable when failAdd is set.
type recordingManager struct {
	manager.Manager
	failIndex string
	failAdd   bool
	indexes   map[string]client.IndexerFunc
}

func newRecordingManager(t *testing.T) *recordingManager {
	t.Helper()
	mgr, err := ctrl.NewManager(&rest.Config{Host: "http://127.0.0.1:1"}, ctrl.Options{
		Scheme:                 newScaleTestScheme(t),
		Metrics:                metricsserver.Options{BindAddress: "0"},
		HealthProbeBindAddress: "0",
		Controller:             config.Controller{SkipNameValidation: new(true)},
	})
	if err != nil {
		t.Fatalf("new manager: %v", err)
	}
	return &recordingManager{Manager: mgr, indexes: map[string]client.IndexerFunc{}}
}

func (m *recordingManager) GetFieldIndexer() client.FieldIndexer { return m }

func (m *recordingManager) IndexField(_ context.Context, _ client.Object, field string, extract client.IndexerFunc) error {
	if field == m.failIndex {
		return errRegistration
	}
	m.indexes[field] = extract
	return nil
}

func (m *recordingManager) Add(runnable manager.Runnable) error {
	if m.failAdd {
		return errRegistration
	}
	return m.Manager.Add(runnable)
}

type controllerSetup struct {
	name string
	// setup registers the controller with scheme, or the manager's when nil.
	setup   func(m *recordingManager, scheme *runtime.Scheme) error
	indexes []string
	// needsKinds is set for a controller that resolves its own kind from the scheme.
	needsKinds bool
}

func schemeOr(m manager.Manager, scheme *runtime.Scheme) *runtime.Scheme {
	if scheme == nil {
		return m.GetScheme()
	}
	return scheme
}

var controllerSetups = []controllerSetup{
	{name: "runtime-artifact", setup: func(m *recordingManager, s *runtime.Scheme) error {
		return (&ArtifactReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s)}).SetupWithManager(m)
	}},
	{name: "workload-host", indexes: []string{workloadByHostIDIndex}, setup: func(m *recordingManager, s *runtime.Scheme) error {
		return (&HostReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s), RetiredHosts: &RetiredHosts{}}).SetupWithManager(m)
	}},
	{name: "host-pod", indexes: []string{hostnameFieldIndex, hostPodIPFieldIndex}, setup: func(m *recordingManager, s *runtime.Scheme) error {
		return (&HostPodReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s), RetiredHosts: &RetiredHosts{}}).SetupWithManager(m)
	}},
	{
		name:    "workload-replica",
		indexes: []string{hostIDIndex, workloadSchedulableHostsIndex, hostEnvironmentIndex},
		setup: func(m *recordingManager, s *runtime.Scheme) error {
			return (&WorkloadReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s)}).SetupWithManager(m)
		},
	},
	{
		name:       "workload-replicaset",
		indexes:    []string{workloadReplicaSetOwnerIndex, workloadReplicaSetHashIndex},
		needsKinds: true,
		setup: func(m *recordingManager, s *runtime.Scheme) error {
			return (&WorkloadReplicaSetReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s)}).SetupWithManager(m)
		},
	},
	{
		name: "workload-deployment", indexes: []string{workloadDeploymentOwnerIndex}, needsKinds: true,
		setup: func(m *recordingManager, s *runtime.Scheme) error {
			return (&WorkloadDeploymentReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s)}).SetupWithManager(m)
		},
	},
	{
		name: "workload-daemonset", indexes: []string{workloadDaemonSetOwnerIndex}, needsKinds: true,
		setup: func(m *recordingManager, s *runtime.Scheme) error {
			return (&WorkloadDaemonSetReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s)}).SetupWithManager(m)
		},
	},
	{name: "workload-route", indexes: []string{workloadKubernetesServiceIndex}, setup: func(m *recordingManager, s *runtime.Scheme) error {
		return (&WorkloadRouteReconciler{Client: m.GetClient(), Scheme: schemeOr(m, s)}).SetupWithManager(m)
	}},
}

func TestSetupWithManagerRegistersIndexes(t *testing.T) {
	for _, c := range controllerSetups {
		t.Run(c.name, func(t *testing.T) {
			m := newRecordingManager(t)
			if err := c.setup(m, nil); err != nil {
				t.Fatalf("SetupWithManager: %v", err)
			}
			got := slices.Sorted(maps.Keys(m.indexes))
			if want := slices.Sorted(slices.Values(c.indexes)); !slices.Equal(got, want) {
				t.Errorf("registered indexes %v, want %v", got, want)
			}
		})
	}
}

// A controller whose index or runnable is refused must not come up without
// it: a List against a missing index fails every reconcile that needs it.
func TestSetupWithManagerReturnsRegistrationFailures(t *testing.T) {
	for _, c := range controllerSetups {
		for _, index := range c.indexes {
			t.Run(c.name+"/index "+index, func(t *testing.T) {
				m := newRecordingManager(t)
				m.failIndex = index
				if err := c.setup(m, nil); !errors.Is(err, errRegistration) {
					t.Errorf("SetupWithManager = %v, want the refused index", err)
				}
			})
		}
		t.Run(c.name+"/runnable", func(t *testing.T) {
			m := newRecordingManager(t)
			m.failAdd = true
			if err := c.setup(m, nil); !errors.Is(err, errRegistration) {
				t.Errorf("SetupWithManager = %v, want the refused runnable", err)
			}
		})
	}
}

func TestSetupWithManagerNeedsItsKindInTheScheme(t *testing.T) {
	for _, c := range controllerSetups {
		if !c.needsKinds {
			continue
		}
		t.Run(c.name, func(t *testing.T) {
			m := newRecordingManager(t)
			if err := c.setup(m, runtime.NewScheme()); err == nil {
				t.Fatal("SetupWithManager succeeded with a scheme that lacks its kind")
			}
			if len(m.indexes) != 0 {
				t.Errorf("registered %v before failing", slices.Sorted(maps.Keys(m.indexes)))
			}
		})
	}
}

func controlledBy(kind string, uid types.UID) metav1.ObjectMeta {
	return metav1.ObjectMeta{OwnerReferences: []metav1.OwnerReference{{
		APIVersion: runtimev1alpha1.GroupVersion.String(), Kind: kind, Name: "owner", UID: uid, Controller: new(true),
	}}}
}

func withService(kubernetes *runtimev1alpha1.KubernetesSpec) *runtimev1alpha1.Workload {
	return &runtimev1alpha1.Workload{Spec: runtimev1alpha1.WorkloadSpec{Kubernetes: kubernetes}}
}

// The indexes are what SetupWithManager registers, not copies of them.
func TestRegisteredIndexesExtract(t *testing.T) {
	m := newRecordingManager(t)
	for _, c := range controllerSetups {
		if err := c.setup(m, nil); err != nil {
			t.Fatalf("set up %s: %v", c.name, err)
		}
	}
	for _, tt := range []struct {
		name  string
		index string
		obj   client.Object
		want  []string
	}{
		{"assigned workload", workloadByHostIDIndex, &runtimev1alpha1.Workload{Status: runtimev1alpha1.WorkloadStatus{HostID: indexedHostID}}, []string{indexedHostID}},
		{"unassigned workload", workloadByHostIDIndex, &runtimev1alpha1.Workload{}, nil},
		{"host by host ID", workloadByHostIDIndex, &runtimev1alpha1.Host{HostID: indexedHostID}, nil},

		{"host with a hostname", hostnameFieldIndex, &runtimev1alpha1.Host{Hostname: "10.0.0.1"}, []string{"10.0.0.1"}},
		{"host without a hostname", hostnameFieldIndex, &runtimev1alpha1.Host{}, nil},
		{"pod by hostname", hostnameFieldIndex, &corev1.Pod{}, nil},

		{"host with an ID", hostIDIndex, &runtimev1alpha1.Host{HostID: indexedHostID}, []string{indexedHostID}},
		{"host without an ID", hostIDIndex, &runtimev1alpha1.Host{}, nil},
		{"workload by host ID", hostIDIndex, &runtimev1alpha1.Workload{Spec: runtimev1alpha1.WorkloadSpec{HostID: indexedHostID}}, nil},

		{"host in an environment", hostEnvironmentIndex, &runtimev1alpha1.Host{Environment: testTenant}, []string{testTenant}},
		{"host in no environment", hostEnvironmentIndex, &runtimev1alpha1.Host{}, nil},
		{"workload by environment", hostEnvironmentIndex, &runtimev1alpha1.Workload{Spec: runtimev1alpha1.WorkloadSpec{Environment: testTenant}}, nil},

		{"workload behind a service", workloadKubernetesServiceIndex, withService(&runtimev1alpha1.KubernetesSpec{
			Service: &runtimev1alpha1.KubernetesServiceRef{Name: "frontend"}}), []string{"frontend"}},
		{"workload without kubernetes", workloadKubernetesServiceIndex, withService(nil), nil},
		{"workload without a service", workloadKubernetesServiceIndex, withService(&runtimev1alpha1.KubernetesSpec{}), nil},
		{"workload with an unnamed service", workloadKubernetesServiceIndex, withService(&runtimev1alpha1.KubernetesSpec{
			Service: &runtimev1alpha1.KubernetesServiceRef{}}), nil},
		{"host by service", workloadKubernetesServiceIndex, &runtimev1alpha1.Host{}, nil},

		{"daemonset's workload", workloadDaemonSetOwnerIndex, &runtimev1alpha1.Workload{ObjectMeta: controlledBy("WorkloadDaemonSet", indexedDaemonSetUID)}, []string{string(indexedDaemonSetUID)}},
		{"replicaset's workload by daemonset", workloadDaemonSetOwnerIndex, &runtimev1alpha1.Workload{ObjectMeta: controlledBy("WorkloadReplicaSet", indexedReplicaSetUID)}, nil},
		{"unowned workload by daemonset", workloadDaemonSetOwnerIndex, &runtimev1alpha1.Workload{}, nil},

		{"deployment's replicaset", workloadDeploymentOwnerIndex, &runtimev1alpha1.WorkloadReplicaSet{ObjectMeta: controlledBy("WorkloadDeployment", indexedDeploymentUID)}, []string{string(indexedDeploymentUID)}},
		{"daemonset's replicaset by deployment", workloadDeploymentOwnerIndex, &runtimev1alpha1.WorkloadReplicaSet{ObjectMeta: controlledBy("WorkloadDaemonSet", indexedDaemonSetUID)}, nil},
		{"unowned replicaset", workloadDeploymentOwnerIndex, &runtimev1alpha1.WorkloadReplicaSet{}, nil},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := m.indexes[tt.index](tt.obj); !slices.Equal(got, tt.want) {
				t.Errorf("%s(%T) = %q, want %q", tt.index, tt.obj, got, tt.want)
			}
		})
	}
}
