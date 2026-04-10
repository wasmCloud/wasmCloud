package wasmbus

import (
	"context"
	"fmt"
	"sync"
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

// NatsConnect connects to a NATS server at the given URL.
// The URL should be in the form of "nats://host:port".
// This helper function sets some default options and calls `nats.Connect`.
func NatsConnect(url string, options ...NatsOption) (*nats.Conn, error) {
	opts := append([]nats.Option{
		nats.PingInterval(1 * time.Minute),    // default is 2m
		nats.MaxPingsOutstanding(1),           // default is 2
		nats.DrainTimeout(5 * time.Second),    // default is 30s
		nats.FlusherTimeout(30 * time.Second), // default is 1m
		nats.Timeout(5 * time.Second),         // default is 2s
		nats.ReconnectWait(1 * time.Second),   // default is 2s
	}, options...)

	nc, err := nats.Connect(url, opts...)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrTransport, err)
	}

	return nc, nil
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
