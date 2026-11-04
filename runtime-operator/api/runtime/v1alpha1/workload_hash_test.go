package v1alpha1

import "testing"

func TestWorkloadReplicaTemplateHashCompatibility(t *testing.T) {
	populated := WorkloadReplicaTemplate{Spec: WorkloadSpec{
		HostSelector: map[string]string{"zone": "east", "arch": "arm64"},
		Environment:  "team-a",
		Components: []WorkloadComponent{{
			Name: "app", Image: "registry.example/app:v1",
			LocalResources: &LocalResources{Config: map[string]string{"z": "<value>", "a": "first"}},
		}},
	}}
	// Hashes must stay stable across operator upgrades to avoid rollouts.
	for _, tc := range []struct {
		name     string
		template WorkloadReplicaTemplate
		want     string
	}{
		{"empty", WorkloadReplicaTemplate{}, "5859f96865"},
		{"populated", populated, "6775d995c4"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := tc.template.Hash(); got != tc.want {
				t.Fatalf("Hash() = %s, want %s", got, tc.want)
			}
		})
	}
	before := populated.Hash()
	populated.Spec.HostSelector = map[string]string{"arch": "arm64", "zone": "east"}
	populated.Spec.Components[0].LocalResources.Config = map[string]string{"a": "first", "z": "<value>"}
	populated.Labels = map[string]string{"team": "a"}
	populated.Annotations = map[string]string{"note": "ignored"}
	if got := populated.Hash(); got != before {
		t.Fatalf("map order or metadata changed hash: %s != %s", got, before)
	}
	populated.Spec.Components[0].Image = "registry.example/app:v2"
	if got := populated.Hash(); got == before {
		t.Fatal("spec change did not change hash")
	}
}
