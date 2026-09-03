package wasmbus

import (
	"strings"
	"testing"
	"time"

	"github.com/nats-io/nats.go"
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

// TestNatsConnectWaitsOutAStartupRace pins the other half of that contract: the
// *first* connection. nats.Connect fails outright when the server is not up
// yet, and the operator is deployed beside its NATS with no ordering between
// them — so a refused first connection is a race to wait out rather than a
// reason to exit and spend a pod restart saying so.
func TestNatsConnectWaitsOutAStartupRace(t *testing.T) {
	type result struct {
		nc  *nats.Conn
		err error
	}
	// Started against a server that does not exist yet.
	done := make(chan result, 1)
	go func() {
		nc, err := NatsConnect(NatsDefaultURL)
		done <- result{nc, err}
	}()

	// Long enough that the connect above has certainly been refused at least
	// once before anything is listening.
	time.Sleep(500 * time.Millisecond)
	defer wasmbustest.MustStartNats(t)()

	select {
	case got := <-done:
		if got.err != nil {
			t.Fatalf("gave up on a NATS that came up during the window: %v", got.err)
		}
		got.nc.Close()
	case <-time.After(30 * time.Second):
		t.Fatal("NatsConnect never connected after the server came up")
	}
}

// The window is for the server not being up yet. An answer that will not
// change has to be reported when it arrives: spending a minute on a malformed
// URL turns an immediate, accurate error into a minute of silence followed by
// that same error, during which the operator looks like it is starting.
func TestNatsConnectDoesNotWaitOutAPermanentFailure(t *testing.T) {
	restore := NatsInitialConnectWindow
	NatsInitialConnectWindow = 30 * time.Second
	defer func() { NatsInitialConnectWindow = restore }()

	started := time.Now()
	_, err := NatsConnect("://not-a-url")
	if err == nil {
		t.Fatal("expected a failure on a malformed URL")
	}
	if elapsed := time.Since(started); elapsed > 5*time.Second {
		t.Fatalf("a malformed URL took %s to report, so it was retried", elapsed)
	}
}

// Waiting is for the startup race. A URL nothing will ever answer has to be
// reported, or the operator runs forever reconciling nothing.
func TestNatsConnectGivesUpOnAServerThatNeverArrives(t *testing.T) {
	restore := NatsInitialConnectWindow
	NatsInitialConnectWindow = time.Second
	defer func() { NatsInitialConnectWindow = restore }()

	started := time.Now()
	_, err := NatsConnect(NatsDefaultURL)
	if err == nil {
		t.Fatal("expected a failure with no NATS running")
	}
	if elapsed := time.Since(started); elapsed < NatsInitialConnectWindow {
		t.Fatalf("gave up after %s, before the %s window was out", elapsed, NatsInitialConnectWindow)
	}
	if !strings.Contains(err.Error(), "no connection within") {
		t.Fatalf("the error should say the window it exhausted, got: %v", err)
	}
}
