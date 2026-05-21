package runtime

import (
	"testing"

	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	corev1 "k8s.io/api/core/v1"
)

// FuzzMergeMaps verifies MergeMaps never panics, never returns nil, and
// preserves last-writer-wins semantics. Nil and empty maps in the variadic
// argument are also verified to be safe on every call.
func FuzzMergeMaps(f *testing.F) {
	f.Add("host", "localhost", "port", "8080")
	f.Add("", "", "", "")
	f.Add("key", "first", "key", "second")
	f.Add("a", "1", "b", "2")

	f.Fuzz(func(t *testing.T, k1, v1, k2, v2 string) {
		// Nil and empty inputs must never panic or return nil.
		if r := MergeMaps(); r == nil {
			t.Error("MergeMaps() returned nil")
		}
		if r := MergeMaps(nil, map[string]string{k1: v1}); r == nil {
			t.Error("MergeMaps(nil, m) returned nil")
		}

		m1 := map[string]string{k1: v1}
		m2 := map[string]string{k2: v2}
		result := MergeMaps(m1, m2)

		if result == nil {
			t.Fatal("MergeMaps returned nil")
		}
		// m2 is last — its value must win on collision.
		if got := result[k2]; got != v2 {
			t.Errorf("result[%q]=%q, want %q (last-writer-wins)", k2, got, v2)
		}
		// k1 survives if not overwritten.
		if k1 != k2 {
			if got := result[k1]; got != v1 {
				t.Errorf("result[%q]=%q, want %q", k1, got, v1)
			}
		}
	})
}

// FuzzTranslatePullPolicy verifies translatePullPolicy never panics and that
// the three canonical Kubernetes pull policies never map to UNSPECIFIED.
func FuzzTranslatePullPolicy(f *testing.F) {
	f.Add(string(corev1.PullAlways))
	f.Add(string(corev1.PullIfNotPresent))
	f.Add(string(corev1.PullNever))
	f.Add("")
	f.Add("UnknownPolicy")
	f.Add("always") // wrong case

	f.Fuzz(func(t *testing.T, policy string) {
		result := translatePullPolicy(corev1.PullPolicy(policy))
		switch corev1.PullPolicy(policy) {
		case corev1.PullAlways, corev1.PullIfNotPresent, corev1.PullNever:
			if result == runtimev2.ImagePullPolicy_IMAGE_PULL_POLICY_UNSPECIFIED {
				t.Errorf("translatePullPolicy(%q) returned UNSPECIFIED for a known policy", policy)
			}
		}
	})
}

// FuzzGetAuthConfigKey verifies getAuthConfigKey never panics and applies the
// docker.io normalization rule correctly for all inputs.
func FuzzGetAuthConfigKey(f *testing.F) {
	f.Add("docker.io")
	f.Add("index.docker.io")
	f.Add("ghcr.io")
	f.Add("")
	f.Add("my.private.registry.example.com")
	f.Add("localhost:5000")

	f.Fuzz(func(t *testing.T, domain string) {
		result := getAuthConfigKey(domain)
		switch domain {
		case "docker.io", "index.docker.io":
			const want = "https://index.docker.io/v1/"
			if result != want {
				t.Errorf("getAuthConfigKey(%q)=%q, want %q", domain, result, want)
			}
		default:
			if result != domain {
				t.Errorf("getAuthConfigKey(%q)=%q, want domain unchanged", domain, result)
			}
		}
	})
}
