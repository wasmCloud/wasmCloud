package v1alpha1

import (
	"os"
	"path/filepath"
	"regexp"
	"testing"

	"gopkg.in/yaml.v3"
)

// TestAllowedHostsPattern exercises the kubebuilder Pattern regex annotated
// on LocalResources.AllowedHosts. The pattern is loaded from the generated
// CRD YAML so the test follows the source of truth — if someone edits the
// +kubebuilder:validation:items:Pattern annotation and re-runs
// `make manifests`, this test re-extracts and re-checks.
//
// kube-apiserver evaluates OpenAPI patterns via kube-openapi/pkg/validation,
// which uses Go's regexp package (RE2). Testing the same regex with the
// stdlib regexp is faithful to admission-time behavior.
func TestAllowedHostsPattern(t *testing.T) {
	pattern := loadAllowedHostsPattern(t)
	re, err := regexp.Compile(pattern)
	if err != nil {
		t.Fatalf("CRD pattern failed to compile as a Go regex: %v\npattern: %s", err, pattern)
	}

	cases := []struct {
		input string
		want  bool
		why   string
	}{
		// --- accepted forms ---
		{"*", true, "literal star = allow all"},
		{"example.com", true, "bare authority"},
		{"example.com:8443", true, "authority with port"},
		{"localhost:8080", true, "single-label host"},
		{"a.b.c.example.com", true, "deeply nested host"},
		{"*.example.com", true, "scheme-less wildcard"},
		{"*.example.com:8443", true, "wildcard with port"},
		{"https://example.com", true, "URL no path"},
		{"https://example.com/", true, "URL bare trailing slash"},
		{"https://example.com:8443", true, "URL with port"},
		{"https://*.example.com", true, "URL wildcard"},
		{"https://*.example.com:8443/", true, "URL wildcard + port + slash"},

		// --- rejected forms ---
		{"", false, "empty"},
		{"*foo", false, "wildcard without leading dot"},
		{"*com", false, "the *com footgun"},
		{"example.com/v1", false, "bare authority + path (no scheme)"},
		{"example.com:notaport", false, "non-numeric port"},
		{"https://example.com/v1", false, "URL with non-root path"},
		{"https://example.com/v1/users", false, "URL with multi-segment path"},
		{"http://", false, "scheme with no host"},
		{":8080", false, "port with no host"},
		{"-example.com", false, "host label starts with hyphen"},
		{"example-.com", false, "host label ends with hyphen"},

		// Note: ECMA-262 (the OpenAPI spec) and RE2 (Go) agree on every
		// construct used in this pattern (anchors, character classes,
		// alternation, quantifiers, simple escapes). No engine-specific
		// behavior to worry about for our cases.
	}

	for _, tc := range cases {
		got := re.MatchString(tc.input)
		if got != tc.want {
			t.Errorf("MatchString(%q) = %v, want %v  // %s", tc.input, got, tc.want, tc.why)
		}
	}
}

// loadAllowedHostsPattern reads the generated `workloads` CRD and extracts
// the pattern on LocalResources.AllowedHosts items from the OpenAPI schema.
// Parsing YAML structurally avoids brittle text matching when the CRD gains
// additional `pattern:` entries.
func loadAllowedHostsPattern(t *testing.T) string {
	t.Helper()
	here, err := os.Getwd()
	if err != nil {
		t.Fatalf("getwd: %v", err)
	}
	// runtime-operator/api/runtime/v1alpha1 → runtime-operator/config/crd/bases
	crdPath := filepath.Join(
		here, "..", "..", "..",
		"config", "crd", "bases",
		"runtime.wasmcloud.dev_workloads.yaml",
	)
	data, err := os.ReadFile(crdPath)
	if err != nil {
		t.Fatalf("read CRD %q: %v", crdPath, err)
	}

	var crd map[string]any
	if err := yaml.Unmarshal(data, &crd); err != nil {
		t.Fatalf("parse CRD YAML %q: %v", crdPath, err)
	}

	pattern, ok := findAllowedHostsPattern(crd)
	if !ok {
		t.Fatalf("could not locate allowedHosts.items.pattern in %s\n"+
			"(did you run `make manifests` after editing workload_types.go?)", crdPath)
	}
	return pattern
}

func findAllowedHostsPattern(crd map[string]any) (string, bool) {
	spec, ok := asMap(crd["spec"])
	if !ok {
		return "", false
	}
	versions, ok := asSlice(spec["versions"])
	if !ok {
		return "", false
	}

	for _, v := range versions {
		version, ok := asMap(v)
		if !ok {
			continue
		}
		schema, ok := asMap(version["schema"])
		if !ok {
			continue
		}
		openAPIV3Schema, ok := asMap(schema["openAPIV3Schema"])
		if !ok {
			continue
		}

		if pattern, ok := lookupAllowedHostsPattern(openAPIV3Schema); ok {
			return pattern, true
		}
	}

	return "", false
}

func lookupAllowedHostsPattern(root map[string]any) (string, bool) {
	path := []string{
		"properties",
		"spec",
		"properties",
		"components",
		"items",
		"properties",
		"localResources",
		"properties",
		"allowedHosts",
		"items",
	}

	node := root
	for _, key := range path {
		next, ok := asMap(node[key])
		if !ok {
			return "", false
		}
		node = next
	}

	pattern, ok := node["pattern"].(string)
	if !ok || pattern == "" {
		return "", false
	}
	return pattern, true
}

func asMap(v any) (map[string]any, bool) {
	m, ok := v.(map[string]any)
	return m, ok
}

func asSlice(v any) ([]any, bool) {
	s, ok := v.([]any)
	return s, ok
}
