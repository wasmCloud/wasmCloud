package wasmbus

import (
	"testing"
	"time"

	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus/wasmbustest"
)

func waitFor(t *testing.T, timeout time.Duration, desc string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("timed out after %s waiting for %s", timeout, desc)
}

// TestNatsConnectReconnectsForever pins the resilience contract: NatsConnect
// must never give up reconnecting. The nats.go default caps reconnection at 60
// attempts and then closes the connection permanently, which would leave the
// operator's heartbeat subscription silently deaf after a long NATS outage.
func TestNatsConnectReconnectsForever(t *testing.T) {
	defer wasmbustest.MustStartNats(t)()

	nc, err := NatsConnect(NatsDefaultURL)
	if err != nil {
		t.Fatal(err)
	}
	defer nc.Close()

	if got := nc.Opts.MaxReconnect; got != -1 {
		t.Fatalf("expected infinite reconnect (MaxReconnect == -1), got %d", got)
	}
}

// TestNatsRecoversFromServerRestart reproduces the incident: the NATS server is
// recycled out from under a live connection. The connection must not close
// permanently while NATS is gone, must reconnect on its own when NATS returns,
// and the subscription must resume delivering messages — no manual restart.
func TestNatsRecoversFromServerRestart(t *testing.T) {
	const subject = "runtime.operator.heartbeat.test"

	stopNats := wasmbustest.MustStartNats(t)

	nc, err := NatsConnect(NatsDefaultURL)
	if err != nil {
		stopNats()
		t.Fatal(err)
	}
	defer nc.Close()
	bus := NewNatsBus(nc)

	sub, err := bus.Subscribe(subject, 10)
	if err != nil {
		stopNats()
		t.Fatal(err)
	}
	defer func() { _ = sub.Drain() }()

	received := make(chan string, 16)
	sub.Handle(func(msg *Message) {
		received <- string(msg.Data)
	})

	publish := func(payload string) {
		t.Helper()
		msg := NewMessage(subject)
		msg.Data = []byte(payload)
		if err := bus.Publish(msg); err != nil {
			t.Fatalf("publish %q: %v", payload, err)
		}
		if err := nc.Flush(); err != nil {
			t.Fatalf("flush %q: %v", payload, err)
		}
	}

	expect := func(payload string) {
		t.Helper()
		select {
		case got := <-received:
			if got != payload {
				t.Fatalf("expected to receive %q, got %q", payload, got)
			}
		case <-time.After(5 * time.Second):
			t.Fatalf("timed out waiting to receive %q", payload)
		}
	}

	// Baseline: heartbeats flow.
	publish("before")
	expect("before")

	// NATS is recycled. The connection drops but must not close permanently.
	stopNats()
	waitFor(t, 10*time.Second, "the connection to register the disconnect", func() bool {
		return !nc.IsConnected()
	})
	if nc.IsClosed() {
		t.Fatal("connection closed permanently on a NATS restart; reconnect must retry forever")
	}

	// NATS comes back on the same address. Register the restarted server via
	// t.Cleanup so it outlives the deferred nc.Close()/sub.Drain() (cleanups run
	// after deferred calls return). The connection must reconnect on its own and
	// the subscription must resume delivering.
	t.Cleanup(wasmbustest.MustStartNats(t))
	waitFor(t, 30*time.Second, "the connection to reconnect after the NATS restart", func() bool {
		return nc.IsConnected()
	})
	publish("after")
	expect("after")
}
