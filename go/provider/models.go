package provider

import (
	"fmt"

	"github.com/nats-io/nkeys"
)

type Topics struct {
	LATTICE_LINK_GET string
	LATTICE_LINK_DEL string
	LATTICE_LINK_PUT string
	LATTICE_SHUTDOWN string
	LATTICE_HEALTH   string
}

func LatticeTopics(h HostData, providerXkey nkeys.KeyPair) Topics {
	// With secrets support in wasmCloud, links are delivered to the link put topic
	// where the topic segment is the XKey provider public key. On wasmCloud host
	// versions before secrets (<1.1.0), the topic segment is the provider key.
	// We can determine the topic segment based on the presence of the host xkey
	// public key and the provider xkey private key.
	var providerLinkPutKey string
	publicKey, err := providerXkey.PublicKey()
	if len(h.HostXKeyPublicKey) == 0 || len(h.ProviderXKeyPrivateKey.Reveal()) == 0 || err != nil {
		providerLinkPutKey = h.ProviderKey
	} else {
		providerLinkPutKey = publicKey
	}

	return Topics{
		LATTICE_LINK_GET: fmt.Sprintf("wasmbus.rpc.%s.%s.linkdefs.get", h.LatticeRPCPrefix, h.ProviderKey),
		LATTICE_LINK_DEL: fmt.Sprintf("wasmbus.rpc.%s.%s.linkdefs.del", h.LatticeRPCPrefix, h.ProviderKey),
		LATTICE_LINK_PUT: fmt.Sprintf("wasmbus.rpc.%s.%s.linkdefs.put", h.LatticeRPCPrefix, providerLinkPutKey),
		LATTICE_HEALTH:   fmt.Sprintf("wasmbus.rpc.%s.%s.health", h.LatticeRPCPrefix, h.ProviderKey),
		LATTICE_SHUTDOWN: fmt.Sprintf("wasmbus.rpc.%s.%s.default.shutdown", h.LatticeRPCPrefix, h.ProviderKey),
	}
}
