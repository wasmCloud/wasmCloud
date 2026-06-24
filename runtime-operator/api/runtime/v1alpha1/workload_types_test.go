package v1alpha1

import (
	"testing"
)

func TestEnsureHostInterface_SameNamespacePackageDifferentName_KeepsSeparate(t *testing.T) {
	spec := &WorkloadSpec{}

	// Add a named "cache" keyvalue interface
	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Interfaces: []string{"store"},
		ConfigLayer: ConfigLayer{
			Config: map[string]string{"backend": "nats"},
		},
	})

	// Add a named "sessions" keyvalue interface (same namespace:package, different name)
	spec.EnsureHostInterface(HostInterface{
		Name:       "sessions",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Interfaces: []string{"store"},
		ConfigLayer: ConfigLayer{
			Config: map[string]string{"backend": "redis"},
		},
	})

	if len(spec.HostInterfaces) != 2 {
		t.Fatalf("expected 2 host interfaces, got %d", len(spec.HostInterfaces))
	}

	if spec.HostInterfaces[0].Name != "cache" {
		t.Errorf("expected first interface name 'cache', got %q", spec.HostInterfaces[0].Name)
	}
	if spec.HostInterfaces[1].Name != "sessions" {
		t.Errorf("expected second interface name 'sessions', got %q", spec.HostInterfaces[1].Name)
	}
	if spec.HostInterfaces[0].Config["backend"] != "nats" {
		t.Errorf("expected first interface backend 'nats', got %q", spec.HostInterfaces[0].Config["backend"])
	}
	if spec.HostInterfaces[1].Config["backend"] != "redis" {
		t.Errorf("expected second interface backend 'redis', got %q", spec.HostInterfaces[1].Config["backend"])
	}
}

func TestEnsureHostInterface_SameNamespacePackageSameName_Merges(t *testing.T) {
	spec := &WorkloadSpec{}

	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Interfaces: []string{"store"},
		ConfigLayer: ConfigLayer{
			Config: map[string]string{"backend": "nats"},
		},
	})

	// Same name+namespace+package => should merge interfaces and config
	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Interfaces: []string{"atomics"},
		ConfigLayer: ConfigLayer{
			Config: map[string]string{"bucket": "cache-kv"},
		},
	})

	if len(spec.HostInterfaces) != 1 {
		t.Fatalf("expected 1 host interface after merge, got %d", len(spec.HostInterfaces))
	}

	iface := spec.HostInterfaces[0]
	if len(iface.Interfaces) != 2 {
		t.Errorf("expected 2 interfaces after merge, got %d", len(iface.Interfaces))
	}
	if !iface.HasInterface("store") {
		t.Error("expected merged interface to have 'store'")
	}
	if !iface.HasInterface("atomics") {
		t.Error("expected merged interface to have 'atomics'")
	}
	if iface.Config["backend"] != "nats" {
		t.Errorf("expected config backend 'nats', got %q", iface.Config["backend"])
	}
	if iface.Config["bucket"] != "cache-kv" {
		t.Errorf("expected config bucket 'cache-kv', got %q", iface.Config["bucket"])
	}
}

func TestEnsureHostInterface_UnnamedBackwardsCompatible(t *testing.T) {
	spec := &WorkloadSpec{}

	// Two unnamed entries with same namespace:package should merge (backwards compatible)
	spec.EnsureHostInterface(HostInterface{
		Namespace:  "wasi",
		Package:    "http",
		Interfaces: []string{"incoming-handler"},
	})

	spec.EnsureHostInterface(HostInterface{
		Namespace:  "wasi",
		Package:    "http",
		Interfaces: []string{"outgoing-handler"},
	})

	if len(spec.HostInterfaces) != 1 {
		t.Fatalf("expected 1 host interface (unnamed merge), got %d", len(spec.HostInterfaces))
	}
	if len(spec.HostInterfaces[0].Interfaces) != 2 {
		t.Errorf("expected 2 interfaces, got %d", len(spec.HostInterfaces[0].Interfaces))
	}
}

func TestEnsureHostInterface_NamedAndUnnamedAreDistinct(t *testing.T) {
	spec := &WorkloadSpec{}

	// Unnamed entry
	spec.EnsureHostInterface(HostInterface{
		Namespace:  "wasi",
		Package:    "keyvalue",
		Interfaces: []string{"store"},
	})

	// Named entry with same namespace:package
	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Interfaces: []string{"store"},
	})

	if len(spec.HostInterfaces) != 2 {
		t.Fatalf("expected 2 host interfaces (named vs unnamed), got %d", len(spec.HostInterfaces))
	}
}

func TestEnsureHostInterface_CompatibleVersionsMergeKeepingMax(t *testing.T) {
	spec := &WorkloadSpec{}

	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Version:    "0.2.1",
		Interfaces: []string{"store"},
	})
	// Same name + semver-compatible version (canonical "0.2") => merge, keep the
	// higher version.
	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Version:    "0.2.6",
		Interfaces: []string{"atomics"},
	})

	if len(spec.HostInterfaces) != 1 {
		t.Fatalf("expected 1 host interface (compatible merge), got %d", len(spec.HostInterfaces))
	}
	if got := spec.HostInterfaces[0].Version; got != "0.2.6" {
		t.Errorf("expected merged version 0.2.6 (max), got %q", got)
	}
	if !spec.HostInterfaces[0].HasInterface("store") || !spec.HostInterfaces[0].HasInterface("atomics") {
		t.Errorf("expected merged interfaces to include store+atomics, got %v", spec.HostInterfaces[0].Interfaces)
	}
}

func TestEnsureHostInterface_IncompatibleVersionsStayDistinct(t *testing.T) {
	spec := &WorkloadSpec{}

	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Version:    "0.2.0",
		Interfaces: []string{"store"},
	})
	// Same name but semver-incompatible (canonical "0.2" vs "0.3") => distinct.
	spec.EnsureHostInterface(HostInterface{
		Name:       "cache",
		Namespace:  "wasi",
		Package:    "keyvalue",
		Version:    "0.3.0",
		Interfaces: []string{"store"},
	})

	if len(spec.HostInterfaces) != 2 {
		t.Fatalf("expected 2 host interfaces (incompatible versions stay distinct), got %d", len(spec.HostInterfaces))
	}
}

func TestCanonVersion(t *testing.T) {
	cases := map[string]string{
		"":            "",
		"1.2.3":       "1",
		"0.2.6-rc.1":  "0.2",
		"0.2.0-draft": "0.2",
		"0.0.1-alpha": "0.0.1",
		"not-semver":  "not-semver",
	}
	for in, want := range cases {
		if got := canonVersion(in); got != want {
			t.Errorf("canonVersion(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestMaxVersion(t *testing.T) {
	cases := []struct{ a, b, want string }{
		{"0.2.1", "0.2.6", "0.2.6"},
		{"0.2.10", "0.2.9", "0.2.10"},
		{"0.3.0", "0.2.9", "0.3.0"},
		{"", "0.2.0", "0.2.0"},
		{"0.2.0", "", "0.2.0"},
	}
	for _, c := range cases {
		if got := maxVersion(c.a, c.b); got != c.want {
			t.Errorf("maxVersion(%q, %q) = %q, want %q", c.a, c.b, got, c.want)
		}
	}
}
