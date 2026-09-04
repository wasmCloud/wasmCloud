package wasmbus

import (
	"context"
	"errors"
	"fmt"
	"net"
	"sync"
	"syscall"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats.go"
)

// NatsBus is a Bus implementation that uses NATS as the transport.
type NatsBus struct {
	nc *nats.Conn
}

const NatsDefaultURL = nats.DefaultURL

var _ Bus = (*NatsBus)(nil)

// NatsOption is an option for configuring a NATS connection.
type NatsOption = nats.Option

// NatsInitialConnectWindow is how long [NatsConnect] keeps retrying a refused
// or timed-out *first* connection before giving up.
//
// Only the first one needs this. Once established, the options below reconnect
// forever and buffer through the gap — but nats.Connect itself fails outright
// if the server is not there yet, and the operator is deployed beside its NATS
// with no ordering between them. Without a window the operator exits, and the
// pod restarts until the race happens to go the other way; that spends the
// restart count, which is the signal something is actually wrong.
//
// A window rather than nats.RetryOnFailedConnect so a genuinely unreachable
// or misconfigured URL is still reported, instead of leaving an operator that
// runs forever and reconciles nothing.
var NatsInitialConnectWindow = 60 * time.Second

// NatsConnect connects to a NATS server at the given URL, waiting out a
// startup race for up to [NatsInitialConnectWindow].
//
// Prefer [NatsConnectContext] anywhere a context is in hand: this one cannot be
// cancelled, so a signal arriving during the wait is not acted on until it ends.
func NatsConnect(url string, options ...NatsOption) (*nats.Conn, error) {
	return NatsConnectContext(context.Background(), url, options...)
}

// NatsConnectContext connects to a NATS server at the given URL.
// The URL should be in the form of "nats://host:port".
// This helper function sets some default options and calls `nats.Connect`,
// retrying a first connection that is refused for up to
// [NatsInitialConnectWindow] or until `ctx` is done.
func NatsConnectContext(ctx context.Context, url string, options ...NatsOption) (*nats.Conn, error) {
	opts := append([]nats.Option{
		nats.PingInterval(1 * time.Minute),    // default is 2m
		nats.MaxPingsOutstanding(1),           // default is 2
		nats.DrainTimeout(5 * time.Second),    // default is 30s
		nats.FlusherTimeout(30 * time.Second), // default is 1m
		nats.Timeout(5 * time.Second),         // default is 2s
		nats.ReconnectWait(1 * time.Second),   // default is 2s
		// Reconnect forever. The nats.go default caps reconnection at 60
		// attempts, after which the connection is closed permanently and any
		// subscriptions (e.g. the operator's host heartbeat watch) go silently
		// deaf with no recovery.
		nats.MaxReconnects(-1),
		nats.ReconnectJitter(100*time.Millisecond, 1*time.Second), // spread reconnect storms
	}, options...)

	deadline := time.Now().Add(NatsInitialConnectWindow)
	backoff := 250 * time.Millisecond
	for {
		nc, err := nats.Connect(url, opts...)
		if err == nil {
			return nc, nil
		}
		if !worthRetrying(err) {
			return nil, fmt.Errorf("%w: %v", ErrTransport, err)
		}
		remaining := time.Until(deadline)
		if remaining <= 0 {
			return nil, fmt.Errorf("%w: %v (no connection within %s)", ErrTransport, err, NatsInitialConnectWindow)
		}
		// Selected on rather than slept through: this runs before the manager
		// starts, so nothing else is watching for a signal, and a plain sleep
		// outlives a grace period shorter than the window.
		timer := time.NewTimer(min(backoff, remaining))
		select {
		case <-ctx.Done():
			timer.Stop()
			return nil, fmt.Errorf("%w: %v (gave up waiting for NATS: %v)", ErrTransport, err, ctx.Err())
		case <-timer.C:
		}
		backoff = min(backoff*2, 5*time.Second)
	}
}

// worthRetrying reports whether a failed connection is the startup race the
// window exists for, rather than an answer that will not change.
//
// Only "the server is not there yet" is waited out. A rejected credential or a
// URL that does not parse reads the same on the last attempt as the first, and
// spending the window on it turns an immediate, accurate error into a minute of
// silence followed by that same error.
func worthRetrying(err error) bool {
	var dnsErr *net.DNSError
	switch {
	case errors.Is(err, nats.ErrNoServers),
		errors.Is(err, nats.ErrTimeout),
		errors.Is(err, syscall.ECONNREFUSED),
		errors.Is(err, syscall.EHOSTUNREACH),
		errors.Is(err, syscall.ENETUNREACH):
		return true
	case errors.As(err, &dnsErr):
		// A Service's record does not exist until the Service does, which is
		// the same race one layer down, and a resolver that is itself starting
		// answers temporarily. Anything else from DNS — a name that cannot be
		// parsed, a refused query — will read the same on the last attempt.
		//
		// A misspelt Service is indistinguishable from one that has not been
		// created yet, so it costs the whole window before it is reported.
		return dnsErr.IsNotFound || dnsErr.IsTemporary
	default:
		return false
	}
}

func NatsDefaultServerOptions() *server.Options {
	return &server.Options{
		ServerName:      "wasmbus",
		Port:            nats.DefaultPort,
		JetStream:       true,
		NoSigs:          true,
		JetStreamDomain: "default",
	}
}

func NatsEmbeddedServer(opts *server.Options, startTimeout time.Duration) (*server.Server, error) {
	s, err := server.NewServer(opts)
	if err != nil {
		return nil, err
	}

	s.Start()

	if !s.ReadyForConnections(startTimeout) {
		s.Shutdown()
		return nil, fmt.Errorf("nats server did not start")
	}

	return s, nil
}

// NewNatsBus creates a new NATS bus using the given NATS connection.
func NewNatsBus(nc *nats.Conn) *NatsBus {
	return &NatsBus{
		nc: nc,
	}
}

// NatsSubscription is a Subscription implementation for NATS.
type NatsSubscription struct {
	ch        chan *nats.Msg
	ns        *nats.Subscription
	bus       Bus
	wg        sync.WaitGroup
	closeOnce sync.Once
}

// Handle implements `Subscription.Handle` for NATS.
// Starts a goroutine to consume messages and returns once the goroutine is ready to receive.
func (s *NatsSubscription) Handle(callback SubscriptionCallback) {
	ready := make(chan struct{})
	s.wg.Add(1)
	go func() {
		defer s.wg.Done()
		close(ready)
		for {
			msg, ok := <-s.ch
			if !ok {
				break
			}
			callback(&Message{
				Subject: msg.Subject,
				Reply:   msg.Reply,
				Header:  Header(msg.Header),
				Data:    msg.Data,
				bus:     s.bus,
			})
		}
	}()
	<-ready
}

// Drain implements `Subscription.Drain` for NATS.
func (s *NatsSubscription) Drain() error {
	err := s.ns.Drain()
	// Wait for the NATS subscription to fully drain before closing the
	// channel, otherwise NATS may try to send on a closed channel.
	if err == nil {
		for s.ns.IsValid() {
			time.Sleep(1 * time.Millisecond)
		}
	}
	s.closeOnce.Do(func() { close(s.ch) })
	s.wg.Wait()
	return err
}

// QueueSubscribe implements `Bus.QueueSubscribe` for NATS.
func (c *NatsBus) QueueSubscribe(subject string, queue string, backlog int) (Subscription, error) {
	ch := make(chan *nats.Msg, backlog)
	sub, err := c.nc.ChanQueueSubscribe(subject, queue, ch)
	if err != nil {
		return nil, err
	}

	return &NatsSubscription{
			ch:  ch,
			ns:  sub,
			bus: c,
		},
		nil
}

// Subscribe implements `Bus.Subscribe` for NATS.
func (c *NatsBus) Subscribe(subject string, backlog int) (Subscription, error) {
	ch := make(chan *nats.Msg, backlog)
	sub, err := c.nc.ChanSubscribe(subject, ch)
	if err != nil {
		return nil, err
	}

	return &NatsSubscription{
			ch:  ch,
			ns:  sub,
			bus: c,
		},
		nil
}

// Request implements `Bus.Request` for NATS.
func (c *NatsBus) Request(ctx context.Context, msg *Message) (*Message, error) {
	reqMsg := nats.NewMsg(msg.Subject)
	reqMsg.Data = msg.Data
	reqMsg.Header = nats.Header(msg.Header)
	respMsg, err := c.nc.RequestMsgWithContext(ctx, reqMsg)
	if err != nil {
		return nil, err
	}

	return &Message{
		Subject: respMsg.Subject,
		Reply:   respMsg.Reply,
		Header:  Header(respMsg.Header),
		Data:    respMsg.Data,
		bus:     c,
	}, nil
}

// Publish implements `Bus.Publish` for NAT
func (c *NatsBus) Publish(msg *Message) error {
	reqMsg := nats.NewMsg(msg.Subject)
	reqMsg.Data = msg.Data
	reqMsg.Header = nats.Header(msg.Header)
	reqMsg.Reply = msg.Reply

	return c.nc.PublishMsg(reqMsg)
}
