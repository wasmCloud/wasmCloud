package v1alpha1

import (
	"testing"
)

const (
	wasiNamespace = "wasi"
	kvCacheName   = "cache"
	kvStore       = "store"
	kvBackend     = "backend"
	natsBackend   = "nats"
)

// kvCache is a named wasi:keyvalue host interface at the given version.
func kvCache(version string, interfaces ...string) HostInterface {
	return HostInterface{
		Name:       kvCacheName,
		Namespace:  wasiNamespace,
		Package:    "keyvalue",
		Version:    version,
		Interfaces: interfaces,
	}
}

func TestEnsureHostInterface_SameNamespacePackageDifferentName_KeepsSeparate(t *testing.T) {
	spec := &WorkloadSpec{}

	// Add a named "cache" keyvalue interface
	cache := kvCache("", kvStore)
	cache.Config = map[string]string{kvBackend: natsBackend}
	spec.EnsureHostInterface(cache)

	// Add a named "sessions" keyvalue interface (same namespace:package, different name)
	sessions := kvCache("", kvStore)
	sessions.Name = "sessions"
	sessions.Config = map[string]string{kvBackend: "redis"}
	spec.EnsureHostInterface(sessions)

	if len(spec.HostInterfaces) != 2 {
		t.Fatalf("expected 2 host interfaces, got %d", len(spec.HostInterfaces))
	}

	if spec.HostInterfaces[0].Name != kvCacheName {
		t.Errorf("expected first interface name 'cache', got %q", spec.HostInterfaces[0].Name)
	}
	if spec.HostInterfaces[1].Name != "sessions" {
		t.Errorf("expected second interface name 'sessions', got %q", spec.HostInterfaces[1].Name)
	}
	if spec.HostInterfaces[0].Config[kvBackend] != natsBackend {
		t.Errorf("expected first interface backend 'nats', got %q", spec.HostInterfaces[0].Config[kvBackend])
	}
	if spec.HostInterfaces[1].Config[kvBackend] != "redis" {
		t.Errorf("expected second interface backend 'redis', got %q", spec.HostInterfaces[1].Config[kvBackend])
	}
}

func TestEnsureHostInterface_SameNamespacePackageSameName_Merges(t *testing.T) {
	spec := &WorkloadSpec{}

	first := kvCache("", kvStore)
	first.Config = map[string]string{kvBackend: natsBackend}
	spec.EnsureHostInterface(first)

	// Same name+namespace+package => should merge interfaces and config
	second := kvCache("", "atomics")
	second.Config = map[string]string{"bucket": "cache-kv"}
	spec.EnsureHostInterface(second)

	if len(spec.HostInterfaces) != 1 {
		t.Fatalf("expected 1 host interface after merge, got %d", len(spec.HostInterfaces))
	}

	iface := spec.HostInterfaces[0]
	if len(iface.Interfaces) != 2 {
		t.Errorf("expected 2 interfaces after merge, got %d", len(iface.Interfaces))
	}
	if !iface.HasInterface(kvStore) {
		t.Error("expected merged interface to have 'store'")
	}
	if !iface.HasInterface("atomics") {
		t.Error("expected merged interface to have 'atomics'")
	}
	if iface.Config[kvBackend] != natsBackend {
		t.Errorf("expected config backend 'nats', got %q", iface.Config[kvBackend])
	}
	if iface.Config["bucket"] != "cache-kv" {
		t.Errorf("expected config bucket 'cache-kv', got %q", iface.Config["bucket"])
	}
}

func TestEnsureHostInterface_UnnamedBackwardsCompatible(t *testing.T) {
	spec := &WorkloadSpec{}

	// Two unnamed entries with same namespace:package should merge (backwards compatible)
	spec.EnsureHostInterface(HostInterface{
		Namespace:  wasiNamespace,
		Package:    "http",
		Interfaces: []string{"incoming-handler"},
	})

	spec.EnsureHostInterface(HostInterface{
		Namespace:  wasiNamespace,
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
	unnamed := kvCache("", kvStore)
	unnamed.Name = ""
	spec.EnsureHostInterface(unnamed)

	// Named entry with same namespace:package
	spec.EnsureHostInterface(kvCache("", kvStore))

	if len(spec.HostInterfaces) != 2 {
		t.Fatalf("expected 2 host interfaces (named vs unnamed), got %d", len(spec.HostInterfaces))
	}
}

func TestEnsureHostInterface_CompatibleVersionsMergeKeepingMax(t *testing.T) {
	spec := &WorkloadSpec{}

	older, newer := "0.2.1", "0.2.6"
	spec.EnsureHostInterface(kvCache(older, kvStore))
	// Same name + semver-compatible version (canonical "0.2") => merge, keep the
	// higher version.
	spec.EnsureHostInterface(kvCache(newer, "atomics"))

	if len(spec.HostInterfaces) != 1 {
		t.Fatalf("expected 1 host interface (compatible merge), got %d", len(spec.HostInterfaces))
	}
	if got := spec.HostInterfaces[0].Version; got != newer {
		t.Errorf("expected merged version %q (max), got %q", newer, got)
	}
	if !spec.HostInterfaces[0].HasInterface(kvStore) || !spec.HostInterfaces[0].HasInterface("atomics") {
		t.Errorf("expected merged interfaces to include store+atomics, got %v", spec.HostInterfaces[0].Interfaces)
	}
}

func TestEnsureHostInterface_IncompatibleVersionsStayDistinct(t *testing.T) {
	spec := &WorkloadSpec{}

	spec.EnsureHostInterface(kvCache("0.2.0", kvStore))
	// Same name but semver-incompatible (canonical "0.2" vs "0.3") => distinct.
	spec.EnsureHostInterface(kvCache("0.3.0", kvStore))

	if len(spec.HostInterfaces) != 2 {
		t.Fatalf("expected 2 host interfaces (incompatible versions stay distinct), got %d", len(spec.HostInterfaces))
	}
}

func TestCanonVersion(t *testing.T) {
	cases := map[string]string{
		"":            "",
		"1.2.3":       "1",
		"0.2.7":       "0.2",
		"0.2.6+build": "0.2",
		"0.0.1":       "0.0.1",
		"0.2.6-rc.1":  "0.2.6-rc.1",
		"0.2.0-draft": "0.2.0-draft",
		"0.0.1-alpha": "0.0.1-alpha",
		"not-semver":  "not-semver",
	}
	for in, want := range cases {
		if got := canonVersion(in); got != want {
			t.Errorf("canonVersion(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestMaxVersion(t *testing.T) {
	// Each case is checked in both argument orders.
	cases := []struct{ lower, higher string }{
		{"0.2.1", "0.2.6"},
		{"0.2.9", "0.2.10"},
		{"0.2.9", "0.3.0"},
		{"", "0.2.0"},
	}
	for _, c := range cases {
		for _, args := range [][2]string{{c.lower, c.higher}, {c.higher, c.lower}} {
			if got := maxVersion(args[0], args[1]); got != c.higher {
				t.Errorf("maxVersion(%q, %q) = %q, want %q", args[0], args[1], got, c.higher)
			}
		}
	}
}
