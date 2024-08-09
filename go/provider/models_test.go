package provider

import (
	"fmt"
	"testing"

	"github.com/nats-io/nkeys"
)

// This test ensures that the LatticeTopics function returns the correct topics for wasmCloud 1.0 and 1.1+.
func TestLatticeTopics(t *testing.T) {
	xkey, err := nkeys.CreateCurveKeys()
	if err != nil {
		t.Errorf("Expected err to be nil, got: %v", err)
	}
	wasmCloudOneDotZero := HostData{ProviderKey: "providerfoo", LatticeRPCPrefix: "lattice123", ProviderXKeyPrivateKey: SecretStringValue{value: ""}, HostXKeyPublicKey: ""}
	OneDotZeroTopics := LatticeTopics(wasmCloudOneDotZero, xkey)

	// Test LATTICE_LINK_GET
	expectedLinkGet := "wasmbus.rpc.lattice123.providerfoo.linkdefs.get"
	if OneDotZeroTopics.LATTICE_LINK_GET != expectedLinkGet {
		t.Errorf("Expected LATTICE_LINK_GET to be %q, got %q", expectedLinkGet, OneDotZeroTopics.LATTICE_LINK_GET)
	}

	// Test LATTICE_LINK_DEL
	expectedLinkDel := "wasmbus.rpc.lattice123.providerfoo.linkdefs.del"
	if OneDotZeroTopics.LATTICE_LINK_DEL != expectedLinkDel {
		t.Errorf("Expected LATTICE_LINK_DEL to be %q, got %q", expectedLinkDel, OneDotZeroTopics.LATTICE_LINK_DEL)
	}

	// Test LATTICE_LINK_PUT
	expectedLinkPut := "wasmbus.rpc.lattice123.providerfoo.linkdefs.put"
	if OneDotZeroTopics.LATTICE_LINK_PUT != expectedLinkPut {
		t.Errorf("Expected LATTICE_LINK_PUT to be %q, got %q", expectedLinkPut, OneDotZeroTopics.LATTICE_LINK_PUT)
	}

	// Test LATTICE_SHUTDOWN
	expectedShutdown := "wasmbus.rpc.lattice123.providerfoo.default.shutdown"
	if OneDotZeroTopics.LATTICE_SHUTDOWN != expectedShutdown {
		t.Errorf("Expected LATTICE_SHUTDOWN to be %q, got %q", expectedShutdown, OneDotZeroTopics.LATTICE_SHUTDOWN)
	}

	// Test LATTICE_HEALTH
	expectedHealth := "wasmbus.rpc.lattice123.providerfoo.health"
	if OneDotZeroTopics.LATTICE_HEALTH != expectedHealth {
		t.Errorf("Expected LATTICE_HEALTH to be %q, got %q", expectedHealth, OneDotZeroTopics.LATTICE_HEALTH)
	}

	// Test secrets / wasmCloud 1.1 and later topics. All are the same as 1.0 except LATTICE_LINK_PUT
	xkeyPublicKey, err := xkey.PublicKey()
	if err != nil {
		t.Errorf("Expected err to be nil, got: %v", err)
	}
	xkeyPrivateKey, err := xkey.Seed()
	if err != nil {
		t.Errorf("Expected err to be nil, got: %v", err)
	}
	wasmCloudOneDotOne := HostData{ProviderKey: "providerfoo", LatticeRPCPrefix: "lattice123", ProviderXKeyPrivateKey: SecretStringValue{value: string(xkeyPrivateKey)}, HostXKeyPublicKey: xkeyPublicKey}
	OneDotOneTopics := LatticeTopics(wasmCloudOneDotOne, xkey)

	// Test LATTICE_LINK_GET
	if OneDotOneTopics.LATTICE_LINK_GET != expectedLinkGet {
		t.Errorf("Expected LATTICE_LINK_GET to be %q, got %q", expectedLinkGet, OneDotOneTopics.LATTICE_LINK_GET)
	}

	// Test LATTICE_LINK_DEL
	if OneDotOneTopics.LATTICE_LINK_DEL != expectedLinkDel {
		t.Errorf("Expected LATTICE_LINK_DEL to be %q, got %q", expectedLinkDel, OneDotOneTopics.LATTICE_LINK_DEL)
	}

	// Test LATTICE_LINK_PUT
	if err != nil {
		t.Errorf("Expected err to be nil, got: %v", err)
	}
	expectedLinkPut = fmt.Sprintf("wasmbus.rpc.lattice123.%s.linkdefs.put", xkeyPublicKey)
	if OneDotOneTopics.LATTICE_LINK_PUT != expectedLinkPut {
		t.Errorf("Expected LATTICE_LINK_PUT to be %q, got %q", expectedLinkPut, OneDotOneTopics.LATTICE_LINK_PUT)
	}

	// Test LATTICE_SHUTDOWN
	if OneDotOneTopics.LATTICE_SHUTDOWN != expectedShutdown {
		t.Errorf("Expected LATTICE_SHUTDOWN to be %q, got %q", expectedShutdown, OneDotOneTopics.LATTICE_SHUTDOWN)
	}

	// Test LATTICE_HEALTH
	if OneDotOneTopics.LATTICE_HEALTH != expectedHealth {
		t.Errorf("Expected LATTICE_HEALTH to be %q, got %q", expectedHealth, OneDotOneTopics.LATTICE_HEALTH)
	}
}
