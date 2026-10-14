package runtime

import (
	"fmt"
	"strings"
	"testing"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestHostInterfaceAdmissionUsesCanonicalVersions(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-interface-admission-test")

	tests := []struct {
		name      string
		versions  [2]string
		wantAdmit bool
	}{
		{name: "incompatible zero-major minors", versions: [2]string{"0.2.0", "0.3.0"}, wantAdmit: true},
		{name: "compatible zero-major minors", versions: [2]string{"0.2.1", "0.2.6"}},
		{name: "compatible stable majors", versions: [2]string{"1.2.3", "1.9.0"}},
		{name: "incompatible stable majors", versions: [2]string{"1.2.3", "2.0.0"}, wantAdmit: true},
		{name: "incompatible zero minor patches", versions: [2]string{"0.0.1", "0.0.2"}, wantAdmit: true},
		{name: "compatible zero minor prereleases", versions: [2]string{"0.0.1-alpha", "0.0.1-beta"}},
		{name: "coerced compatible versions", versions: [2]string{"v0.02", "0.2.6"}},
		{name: "distinct invalid versions", versions: [2]string{"not-semver-a", "not-semver-b"}, wantAdmit: true},
	}

	for i, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			workload := &runtimev1alpha1.Workload{
				ObjectMeta: metav1.ObjectMeta{
					Name:      fmt.Sprintf("host-interface-versions-%d", i),
					Namespace: ns,
				},
				Spec: runtimev1alpha1.WorkloadSpec{
					HostInterfaces: []runtimev1alpha1.HostInterface{
						{
							Namespace:  "wasi",
							Package:    "http",
							Interfaces: []string{"handler"},
							Version:    tt.versions[0],
						},
						{
							Namespace:  "wasi",
							Package:    "http",
							Interfaces: []string{"outgoing-handler"},
							Version:    tt.versions[1],
						},
					},
				},
			}

			err := c.Create(ctx, workload)
			if tt.wantAdmit {
				if err != nil {
					t.Fatalf("expected versions %q to be admitted: %v", tt.versions, err)
				}
				return
			}
			if !apierrors.IsInvalid(err) {
				t.Fatalf("expected versions %q to be rejected as invalid, got: %v", tt.versions, err)
			}
			if !strings.Contains(err.Error(), "semver-compatible version") {
				t.Fatalf("expected hostInterface compatibility error, got: %v", err)
			}
		})
	}
}
