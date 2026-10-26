package natsclient

import (
	"testing"

	"github.com/nats-io/nats-server/v2/server"

	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// TestNatsLivenessCheck verifies liveness reflects NATS connectivity: healthy
// while connected, and unhealthy once the connection is permanently closed so
// the kubelet restarts the pod.
func TestNatsLivenessCheck(t *testing.T) {
	url := startNats(t, func(*server.Options) {})

	nc, err := wasmbus.NatsConnect(url)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	defer nc.Close()

	check := LivenessCheck(nc)

	if err := check(nil); err != nil {
		t.Fatalf("expected healthy while connected, got: %v", err)
	}

	nc.Close()
	if !waitFor(nc.IsClosed) {
		t.Fatal("the connection did not close")
	}
	if err := check(nil); err == nil {
		t.Fatal("expected liveness to fail on a permanently closed nats connection")
	}
}
