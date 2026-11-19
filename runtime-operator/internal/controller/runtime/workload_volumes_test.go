package runtime

import (
	"context"
	"strings"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
)

const (
	mountedVolume = "data"
	mountPath     = "/mnt/data"
	hostDirVolume = "host-dir"
)

func volumeWorkload(volumes []runtimev1alpha1.Volume, component, service []runtimev1alpha1.VolumeMount) *runtimev1alpha1.Workload {
	w := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "volumes", Namespace: testNamespace},
		Spec: runtimev1alpha1.WorkloadSpec{
			Volumes: volumes,
			Components: []runtimev1alpha1.WorkloadComponent{{
				Name: "http", Image: "example.com/http:1",
				LocalResources: &runtimev1alpha1.LocalResources{VolumeMounts: component},
			}},
		},
	}
	if service != nil {
		w.Spec.Service = &runtimev1alpha1.WorkloadService{
			Image:          "example.com/service:1",
			LocalResources: &runtimev1alpha1.LocalResources{VolumeMounts: service},
		}
	}
	return w
}

func volumeReconciler(t *testing.T) *WorkloadReconciler {
	t.Helper()
	return &WorkloadReconciler{Client: fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).Build()}
}

// A mount of a volume the spec does not define would start the component
// without it, so the Workload is refused before any host is chosen, naming
// who mounts what.
func TestMaterializeRefusesMountOfUndefinedVolume(t *testing.T) {
	mounts := []runtimev1alpha1.VolumeMount{{Name: "undefined", MountPath: mountPath}}
	for name, tc := range map[string]struct {
		workload *runtimev1alpha1.Workload
		want     string
	}{
		"in a component": {volumeWorkload(nil, mounts, nil), `component "http" mounts volume "undefined"`},
		"in the service": {volumeWorkload([]runtimev1alpha1.Volume{{Name: mountedVolume}}, nil, mounts),
			`service mounts volume "undefined"`},
	} {
		t.Run(name, func(t *testing.T) {
			r := volumeReconciler(t)
			if err := r.reconcileConfig(context.Background(), tc.workload); err == nil ||
				!strings.Contains(err.Error(), tc.want) {
				t.Fatalf("reconcileConfig returned %v, want an error containing %s", err, tc.want)
			}
		})
	}
}

// Each volume reaches the host with its type set, including the documented
// ephemeral default, so the host never has to guess.
func TestMaterializeVolumeTypes(t *testing.T) {
	mounts := []runtimev1alpha1.VolumeMount{{Name: mountedVolume, MountPath: mountPath}}
	w := volumeWorkload([]runtimev1alpha1.Volume{
		{Name: mountedVolume},
		{Name: "scratch", EphemeralVolume: &runtimev1alpha1.EphemeralVolume{}},
		{Name: hostDirVolume, HostPathVolume: &runtimev1alpha1.HostPathVolume{Path: "/srv/host"}},
	}, mounts, mounts)

	spec, err := volumeReconciler(t).materializeWorkload(context.Background(), w)
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]*runtimev2.Volume{}
	for _, v := range spec.GetVolumes() {
		got[v.GetName()] = v
	}
	for _, name := range []string{mountedVolume, "scratch"} {
		if got[name].GetEmptyDir() == nil {
			t.Errorf("volume %q sent as %v, want an emptyDir", name, got[name])
		}
	}
	if got[hostDirVolume].GetHostPath().GetLocalPath() != "/srv/host" {
		t.Errorf("volume %s sent as %v", hostDirVolume, got[hostDirVolume])
	}
}
