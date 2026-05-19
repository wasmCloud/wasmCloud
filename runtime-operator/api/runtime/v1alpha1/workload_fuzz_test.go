package v1alpha1

import (
	"encoding/json"
	"testing"
)

// FuzzEnsureHostInterface verifies EnsureHostInterface never panics and
// maintains two invariants under arbitrary inputs:
//
//  1. Same (namespace, package, name) key → interfaces are merged, count stays
//     1, and config entries from both calls are present.
//  2. Different key → a new entry is appended, count grows to 2.
func FuzzEnsureHostInterface(f *testing.F) {
	// Interface merge — same key.
	f.Add("wasi", "keyvalue", "cache", "store", "ck", "cv", "wasi", "keyvalue", "cache", "atomics")
	// Unnamed backwards-compatible merge.
	f.Add("wasi", "http", "", "incoming-handler", "", "", "wasi", "http", "", "outgoing-handler")
	// Different name — must stay separate.
	f.Add("wasi", "keyvalue", "a", "store", "k", "v", "wasi", "keyvalue", "b", "store")
	// Entirely different namespace+package.
	f.Add("ns1", "pkg1", "n1", "iface1", "", "", "ns2", "pkg2", "n2", "iface2")
	// All empty.
	f.Add("", "", "", "", "", "", "", "", "", "")

	f.Fuzz(func(t *testing.T,
		ns1, pkg1, name1, iface1, configKey, configVal string,
		ns2, pkg2, name2, iface2 string,
	) {
		spec := &WorkloadSpec{}

		spec.EnsureHostInterface(HostInterface{
			Namespace:  ns1,
			Package:    pkg1,
			Name:       name1,
			Interfaces: []string{iface1},
			ConfigLayer: ConfigLayer{
				Config: map[string]string{configKey: configVal},
			},
		})
		spec.EnsureHostInterface(HostInterface{
			Namespace:  ns2,
			Package:    pkg2,
			Name:       name2,
			Interfaces: []string{iface2},
		})

		sameKey := ns1 == ns2 && pkg1 == pkg2 && name1 == name2
		if sameKey {
			if len(spec.HostInterfaces) != 1 {
				t.Errorf("same key: expected 1 HostInterface, got %d", len(spec.HostInterfaces))
			}
			merged := spec.HostInterfaces[0]
			if !merged.HasInterface(iface1) {
				t.Errorf("merged interface missing %q", iface1)
			}
			if !merged.HasInterface(iface2) {
				t.Errorf("merged interface missing %q", iface2)
			}
			if merged.Config == nil {
				t.Fatal("Config is nil after merge")
			}
			if _, ok := merged.Config[configKey]; !ok {
				t.Errorf("config missing key %q after merge", configKey)
			}
		} else {
			if len(spec.HostInterfaces) != 2 {
				t.Errorf("different key: expected 2 HostInterfaces, got %d", len(spec.HostInterfaces))
			}
		}
	})
}

// FuzzWorkloadSpecJSON verifies that unmarshaling arbitrary bytes into
// WorkloadSpec never panics, and that any successfully-decoded spec can be
// hashed without panic and produces a non-empty result.
//
// This is the primary fuzzing surface for the operator's core data type:
// the API server can deliver unusual-but-valid JSON that no human would write
// as a test case.
func FuzzWorkloadSpecJSON(f *testing.F) {
	f.Add([]byte(`{"environment":"dev","components":[{"name":"c","image":"nginx"}]}`))
	f.Add([]byte(`{}`))
	f.Add([]byte(`null`))
	f.Add([]byte(`{"hostInterfaces":[{"namespace":"wasi","package":"http","interfaces":["incoming-handler"]}]}`))
	f.Add([]byte(`{"volumes":[{"name":"tmp","ephemeral":{}}]}`))

	f.Fuzz(func(t *testing.T, specJSON []byte) {
		spec := &WorkloadSpec{}
		if err := json.Unmarshal(specJSON, spec); err != nil {
			return // malformed JSON is expected and fine
		}

		tmpl := WorkloadReplicaTemplate{Spec: *spec}
		if h := tmpl.Hash(); h == "" {
			t.Error("Hash returned empty string for valid spec")
		}
	})
}
