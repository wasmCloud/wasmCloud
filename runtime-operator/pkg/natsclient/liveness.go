package natsclient

import (
	"errors"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/nats-io/nats.go"
)

// DefaultAuthFailureGrace outlasts the kubelet's refresh of a mounted Secret,
// which by default takes up to its one-minute sync plus a one-minute cache TTL.
const DefaultAuthFailureGrace = 5 * time.Minute

// Liveness reports when a connection can no longer recover on its own: once it
// is closed, or once the server has kept refusing its credential for longer
// than the grace period without a connect in between. Reconnecting for any
// other reason stays healthy, so a NATS rollout does not restart the client.
//
// IgnoreAuthErrorAbort keeps a client retrying a refused credential so it can
// pick up a rotated Secret. A credential still refused after the grace will
// not be replaced by a refresh, and a restart at least surfaces it.
type Liveness struct {
	grace time.Duration
	now   func() time.Time

	mu   sync.Mutex
	conn *nats.Conn
	// The current run of refusals: when it began, and its latest refusal.
	refusedSince time.Time
	lastRefused  time.Time
	lastErr      error
}

// NewLiveness fails once the credential has been refused for longer than
// grace. Zero means DefaultAuthFailureGrace.
func NewLiveness(grace time.Duration) *Liveness {
	if grace <= 0 {
		grace = DefaultAuthFailureGrace
	}
	return &Liveness{grace: grace, now: time.Now}
}

// Option records every refusal and reconnect, rather than only those a probe
// happens to sample. It wraps any error and reconnect handlers set by the
// options before it, so append it last.
func (l *Liveness) Option() nats.Option {
	return func(o *nats.Options) error {
		onError, onReconnect := o.AsyncErrorCB, o.ReconnectedCB
		o.AsyncErrorCB = func(nc *nats.Conn, sub *nats.Subscription, err error) {
			l.refused(err)
			if onError != nil {
				onError(nc, sub, err)
			}
		}
		o.ReconnectedCB = func(nc *nats.Conn) {
			l.connected()
			if onReconnect != nil {
				onReconnect(nc)
			}
		}
		return nil
	}
}

// Watch sets the connection Check reports on, which must have been opened
// with Option.
func (l *Liveness) Watch(conn *nats.Conn) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.conn = conn
}

// Check is a controller-runtime healthz.Checker.
func (l *Liveness) Check(_ *http.Request) error {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.conn == nil {
		return nil
	}
	if l.conn.IsClosed() {
		return errors.New("nats connection permanently closed")
	}
	if l.conn.IsConnected() {
		l.reset()
		return nil
	}
	return l.refusedTooLong()
}

// refusedTooLong fails while the current run of refusals is still going and
// has lasted longer than the grace.
func (l *Liveness) refusedTooLong() error {
	if l.refusedSince.IsZero() || l.now().Sub(l.lastRefused) > l.grace {
		return nil
	}
	if refused := l.lastRefused.Sub(l.refusedSince); refused > l.grace {
		return fmt.Errorf("nats server has refused the credential for %s: %w", refused.Round(time.Second), l.lastErr)
	}
	return nil
}

// refused records an auth error. One more than the grace after the last
// starts a new run: whatever kept the client from the server in between,
// such as an outage, was not the credential.
func (l *Liveness) refused(err error) {
	if !isAuthError(err) {
		return
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.now()
	if l.refusedSince.IsZero() || now.Sub(l.lastRefused) > l.grace {
		l.refusedSince = now
	}
	l.lastRefused = now
	l.lastErr = err
}

func (l *Liveness) connected() {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.reset()
}

func (l *Liveness) reset() {
	l.refusedSince, l.lastRefused, l.lastErr = time.Time{}, time.Time{}, nil
}
