package crdtools

import (
	"sort"
	"testing"

	corev1 "k8s.io/api/core/v1"
)

// FuzzMergeLabels verifies MergeLabels never panics, never returns nil, and
// upholds last-writer-wins semantics for two- and three-map merges.
func FuzzMergeLabels(f *testing.F) {
	f.Add("app", "myapp", "env", "prod", "tier", "backend")
	f.Add("", "", "", "", "", "")
	f.Add("a/b", "val1", "a/b", "val2", "a/b", "val3")
	f.Add("k8s.io/arch", "amd64", "k8s.io/os", "linux", "k8s.io/arch", "arm64")

	f.Fuzz(func(t *testing.T, k1, v1, k2, v2, k3, v3 string) {
		m1 := map[string]string{k1: v1}
		m2 := map[string]string{k2: v2}
		m3 := map[string]string{k3: v3}

		two := MergeLabels(m1, m2)
		three := MergeLabels(m1, m2, m3)

		if two == nil || three == nil {
			t.Fatal("MergeLabels returned nil")
		}

		// Last map wins for two-way merge.
		if got := two[k2]; got != v2 {
			t.Errorf("two[%q]=%q, want %q (last-writer-wins)", k2, got, v2)
		}
		// k1 survives if not overwritten.
		if k1 != k2 {
			if got := two[k1]; got != v1 {
				t.Errorf("two[%q]=%q, want %q", k1, got, v1)
			}
		}

		// Last map wins for three-way merge.
		if got := three[k3]; got != v3 {
			t.Errorf("three[%q]=%q, want %q (last-writer-wins)", k3, got, v3)
		}
	})
}

// FuzzMergeEnvVar verifies MergeEnvVar never panics, deduplicates by name
// (last writer wins), and always returns a sorted result.
func FuzzMergeEnvVar(f *testing.F) {
	f.Add("HOME", "/root", "HOME", "/home/user")
	f.Add("", "", "", "")
	f.Add("A", "1", "B", "2")
	f.Add("PATH", "/usr/bin", "GOPATH", "/go")

	f.Fuzz(func(t *testing.T, name1, val1, name2, val2 string) {
		slice1 := []corev1.EnvVar{{Name: name1, Value: val1}}
		slice2 := []corev1.EnvVar{{Name: name2, Value: val2}}

		result := MergeEnvVar(slice1, slice2)

		// Names must be unique.
		seen := make(map[string]int)
		for _, ev := range result {
			seen[ev.Name]++
			if seen[ev.Name] > 1 {
				t.Errorf("duplicate name %q in result", ev.Name)
			}
		}

		// Result must be sorted by name.
		names := make([]string, len(result))
		for i, ev := range result {
			names[i] = ev.Name
		}
		if !sort.StringsAreSorted(names) {
			t.Errorf("result is not sorted: %v", names)
		}

		// Last writer wins on collision.
		if name1 == name2 {
			for _, ev := range result {
				if ev.Name == name2 && ev.Value != val2 {
					t.Errorf("last-writer-wins violated: got %q, want %q", ev.Value, val2)
				}
			}
		}
	})
}
