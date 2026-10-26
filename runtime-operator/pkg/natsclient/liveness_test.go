package natsclient

import (
	"os"
	"sync"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats.go"

	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// fakeClock is read from the client's callback goroutine and set by the test.
type fakeClock struct {
	mu  sync.Mutex
	now time.Time
}

func (c *fakeClock) Now() time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.now
}

func (c *fakeClock) Add(d time.Duration) time.Time {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.now = c.now.Add(d)
	return c.now
}

func newTestLiveness(grace time.Duration) (*Liveness, *fakeClock) {
	clock := &fakeClock{now: time.Now()}
	liveness := NewLiveness(grace)
	liveness.now = clock.Now
	return liveness, clock
}

// TestNatsLivenessCheck verifies liveness reflects NATS connectivity: healthy
// while connected, and unhealthy once the connection is permanently closed so
// the kubelet restarts the pod.
func TestNatsLivenessCheck(t *testing.T) {
	url := startNats(t, func(*server.Options) {})

	liveness := NewLiveness(DefaultAuthFailureGrace)
	nc, err := wasmbus.NatsConnect(url, liveness.Option())
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	defer nc.Close()
	liveness.Watch(nc)

	if err := liveness.Check(nil); err != nil {
		t.Fatalf("expected healthy while connected, got: %v", err)
	}

	nc.Close()
	if !waitFor(nc.IsClosed) {
		t.Fatal("the connection did not close")
	}
	if err := liveness.Check(nil); err == nil {
		t.Fatal("expected liveness to fail on a permanently closed nats connection")
	}
}

// An embedder that never registered the flags still gets the default grace,
// rather than failing on the first refusal.
func TestNatsLivenessZeroGraceIsTheDefault(t *testing.T) {
	if got := NewLiveness(0).grace; got != DefaultAuthFailureGrace {
		t.Fatalf("grace = %s, want %s", got, DefaultAuthFailureGrace)
	}
}

// The server can rotate a token the mounted Secret has not caught up with.
// Liveness tolerates the refusals for the grace period, fails after it, and
// recovers once the rotated token arrives.
func TestNatsRefusedCredentialFailsLivenessAfterGrace(t *testing.T) {
	configure := func(token string) func(*server.Options) {
		return func(o *server.Options) { o.Authorization = token }
	}
	s := startNatsServer(t, configure("first-token"))
	tokenFile := writeSecret(t, "token", "first-token\n")
	options, err := Flags{TokenFile: tokenFile}.Options()
	if err != nil {
		t.Fatal(err)
	}
	const grace = 5 * time.Minute
	liveness, clock := newTestLiveness(grace)
	nc, err := nats.Connect(s.ClientURL(), append(options,
		nats.MaxReconnects(-1),
		nats.ReconnectWait(20*time.Millisecond),
		nats.ReconnectJitter(0, 0),
		liveness.Option(),
	)...)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	t.Cleanup(nc.Close)
	liveness.Watch(nc)
	if err := liveness.Check(nil); err != nil {
		t.Fatalf("connected: %v", err)
	}

	// Each step waits for a refusal at the new time, so the run is unbroken.
	refusedAt := func(at time.Time) bool {
		return waitFor(func() bool {
			liveness.mu.Lock()
			defer liveness.mu.Unlock()
			return liveness.lastRefused.Equal(at)
		})
	}
	if err := s.ReloadOptions(natsServerOptions(configure("second-token"))); err != nil {
		t.Fatalf("rotate the server token: %v", err)
	}
	if !refusedAt(clock.Now()) {
		t.Fatalf("the stale token was not refused: status %v, last error %v", nc.Status(), nc.LastError())
	}
	if err := liveness.Check(nil); err != nil {
		t.Fatalf("first refusal: %v", err)
	}
	for range 2 {
		if !refusedAt(clock.Add(grace / 2)) {
			t.Fatal("no refusal within the grace")
		}
	}
	if err := liveness.Check(nil); err != nil {
		t.Fatalf("refused for the whole grace: %v", err)
	}
	if !refusedAt(clock.Add(time.Second)) {
		t.Fatal("no refusal past the grace")
	}
	if err := liveness.Check(nil); !isAuthError(err) {
		t.Fatalf("refused past the grace: got %v, want an auth error", err)
	}

	if err := os.WriteFile(tokenFile, []byte("second-token\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if !waitFor(nc.IsConnected) {
		t.Fatalf("no reconnect with the rotated token: status %v", nc.Status())
	}
	if err := liveness.Check(nil); err != nil {
		t.Fatalf("reconnected: %v", err)
	}
}

// Refusals separated by more than the grace, such as by an outage, are two
// runs rather than one long one.
func TestNatsRefusalsAcrossAGapStartOver(t *testing.T) {
	const grace = time.Minute
	liveness, clock := newTestLiveness(grace)

	liveness.refused(nats.ErrAuthorization)
	clock.Add(grace + time.Second)
	liveness.refused(nats.ErrAuthorization)
	if err := liveness.refusedTooLong(); err != nil {
		t.Fatalf("a refusal after an outage: %v", err)
	}
	clock.Add(grace / 2)
	liveness.refused(nats.ErrAuthorization)
	if err := liveness.refusedTooLong(); err != nil {
		t.Fatalf("half a grace into the new run: %v", err)
	}
	clock.Add(grace/2 + time.Second)
	liveness.refused(nats.ErrAuthorization)
	if err := liveness.refusedTooLong(); !isAuthError(err) {
		t.Fatalf("past the grace in one run: got %v, want an auth error", err)
	}

	// Refusals that stop, with no reconnect, are not a credential to restart for.
	clock.Add(grace + time.Second)
	if err := liveness.refusedTooLong(); err != nil {
		t.Fatalf("refusals stopped: %v", err)
	}
}

// A reconnect ends the run even when no probe saw the connection up.
func TestNatsReconnectEndsTheRun(t *testing.T) {
	const grace = time.Minute
	liveness, clock := newTestLiveness(grace)

	liveness.refused(nats.ErrAuthExpired)
	clock.Add(grace / 2)
	liveness.connected()
	clock.Add(grace / 2)
	liveness.refused(nats.ErrAuthExpired)
	clock.Add(grace / 2)
	liveness.refused(nats.ErrAuthExpired)
	if err := liveness.refusedTooLong(); err != nil {
		t.Fatalf("refused for less than the grace since the reconnect: %v", err)
	}
	liveness.refused(nats.ErrTimeout)
	if !liveness.lastRefused.Equal(clock.Now()) || liveness.lastErr != nats.ErrAuthExpired {
		t.Fatal("a non-auth error was recorded as a refusal")
	}
}

// Option keeps the handlers set before it.
func TestNatsLivenessOptionWrapsHandlers(t *testing.T) {
	var errored, reconnected bool
	var opts nats.Options
	for _, option := range []nats.Option{
		nats.ErrorHandler(func(*nats.Conn, *nats.Subscription, error) { errored = true }),
		nats.ReconnectHandler(func(*nats.Conn) { reconnected = true }),
		NewLiveness(time.Minute).Option(),
	} {
		if err := option(&opts); err != nil {
			t.Fatal(err)
		}
	}
	opts.AsyncErrorCB(nil, nil, nats.ErrAuthorization)
	opts.ReconnectedCB(nil)
	if !errored || !reconnected {
		t.Fatalf("errored %v, reconnected %v: want both", errored, reconnected)
	}
}

// Reconnecting for any reason but a refused credential stays healthy however
// long it lasts, so a NATS outage does not restart the client.
func TestNatsOutageKeepsLiveness(t *testing.T) {
	s := startNatsServer(t, func(*server.Options) {})
	liveness, clock := newTestLiveness(time.Minute)
	nc, err := nats.Connect(s.ClientURL(),
		nats.MaxReconnects(-1),
		nats.ReconnectWait(20*time.Millisecond),
		liveness.Option(),
	)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	t.Cleanup(nc.Close)
	liveness.Watch(nc)

	s.Shutdown()
	if !waitFor(func() bool { return !nc.IsConnected() }) {
		t.Fatal("still connected to a stopped server")
	}
	if err := liveness.Check(nil); err != nil {
		t.Fatalf("outage: %v", err)
	}
	clock.Add(time.Hour)
	if err := liveness.Check(nil); err != nil {
		t.Fatalf("an hour-long outage: %v", err)
	}
}
