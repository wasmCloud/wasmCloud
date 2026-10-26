package natsclient

import (
	"errors"
	"net/http"

	"github.com/nats-io/nats.go"
)

// LivenessCheck is a controller-runtime healthz.Checker that fails only once
// nc is permanently closed. It stays healthy while reconnecting so a NATS
// rollout does not restart the client.
func LivenessCheck(nc *nats.Conn) func(*http.Request) error {
	return func(_ *http.Request) error {
		if nc.IsClosed() {
			return errors.New("nats connection permanently closed")
		}
		return nil
	}
}
