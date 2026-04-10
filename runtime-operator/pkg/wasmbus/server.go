package wasmbus

import (
	"context"
	"errors"
	"fmt"
	"sync"
)

// ServerError carries information about transport & encoding errors outside Request/Response scope.
type ServerError struct {
	Context context.Context
	Err     error
	Request *Message
}

// Server is a higher-level abstraction that can be used to register handlers for specific subjects.
// See `AnyServerHandler` for more information.
type Server struct {
	Bus
	// Lattice is an informative field containing the lattice name.
	// It is NOT used when manipulating subjects.
	Lattice string
	// ContextFunc is a function that returns a new context for each message.
	// Defaults to `context.Background`.
	ContextFunc func() context.Context

	subscriptions []Subscription
	lock          sync.Mutex
	errorStream   chan *ServerError
}

// NewServer returns a new server instance.
func NewServer(bus Bus, lattice string) *Server {
	return &Server{
		Bus:         bus,
		Lattice:     lattice,
		ContextFunc: func() context.Context { return context.Background() },
		errorStream: make(chan *ServerError),
	}
}

// ErrorStream returns a channel that can be used to listen for Transport / Encoding level errors.
// See `ServerError` for more information.
func (s *Server) ErrorStream() <-chan *ServerError {
	return s.errorStream
}

func (s *Server) reportError(ctx context.Context, req *Message, err error) {
	select {
	// We don't want to block the server if the error stream is full or nobody is listening
	case s.errorStream <- &ServerError{Context: ctx, Err: err, Request: req}:
	default:
	}
}

// Drain walks through all subscriptions and drains them.
// It also closes the error stream.
// This is a blocking operation.
func (s *Server) Drain() error {
	s.lock.Lock()
	defer s.lock.Unlock()

	var errs []error //nolint:prealloc
	for _, sub := range s.subscriptions {
		errs = append(errs, sub.Drain())
	}
	s.subscriptions = nil
	close(s.errorStream)

	return errors.Join(errs...)
}

// AnyServerHandler is an interface that can be implemented by any handler that can be registered with a server.
// Primary implementations are `RequestHandler` and `ServerHandlerFunc`.
type AnyServerHandler interface {
	HandleMessage(ctx context.Context, msg *Message) error
}

// ServerHandlerFunc is a function type that can be used to implement a server handler from a function.
type ServerHandlerFunc func(context.Context, *Message) error

func (f ServerHandlerFunc) HandleMessage(ctx context.Context, msg *Message) error {
	return f(ctx, msg)
}

// RegisterHandler registers a handler for a given subject.
// Each handler gets their channel subscription with no backlog, and their own goroutine for queue consumption.
// Callers should handle concurrency and synchronization themselves.
func (s *Server) RegisterHandler(subject string, handler AnyServerHandler) error {
	sub, err := s.Subscribe(subject, NoBackLog)
	if err != nil {
		return err
	}
	sub.Handle(func(msg *Message) {
		ctx := s.ContextFunc()
		if err := handler.HandleMessage(ctx, msg); err != nil {
			s.reportError(ctx, msg, err)
		}
	})

	s.lock.Lock()
	defer s.lock.Unlock()
	s.subscriptions = append(s.subscriptions, sub)

	return nil
}

// NewRequestHandler returns a new server handler instance.
// The `T` and `Y` types are used to define the Request and Response types. Both should be structs.
// They will be used as template for request/responses.
func NewRequestHandler[T any, Y any](
	req T,
	resp Y,
	handler func(context.Context, *T) (*Y, error),
) *RequestHandler[T, Y] {
	return &RequestHandler[T, Y]{
		Request:  req,
		Response: resp,
		Handler:  handler,
	}
}

// RequestHandler is a generic handler that can be used to implement a server handler.
// It encodes the logic for handling a message and sending a response.
type RequestHandler[T any, Y any] struct {
	Request     T
	Response    Y
	PreRequest  func(context.Context, *T, *Message) error
	PostRequest func(context.Context, *Y, *Message) error
	Handler     func(context.Context, *T) (*Y, error)
}

// HandleMessage implements the `AnyServerHandler` interface.
func (s *RequestHandler[T, Y]) HandleMessage(ctx context.Context, msg *Message) error {
	req := s.Request
	err := Decode(msg, &req)
	if err != nil {
		return fmt.Errorf("%w: %s", ErrDecode, err)
	}

	if s.PreRequest != nil {
		if err := s.PreRequest(ctx, &req, msg); err != nil {
			return fmt.Errorf("%w: %s", ErrOperation, err)
		}
	}

	resp, err := s.Handler(ctx, &req)
	if err != nil {
		return fmt.Errorf("%w: %s", ErrOperation, err)
	}

	rawResp, err := Encode(msg.Reply, resp)
	if err != nil {
		return fmt.Errorf("%w: %s", ErrEncode, err)
	}
	rawResp.bus = msg.bus

	if s.PostRequest != nil {
		if err := s.PostRequest(ctx, resp, rawResp); err != nil {
			return fmt.Errorf("%w: %s", ErrOperation, err)
		}
	}

	if err := msg.Bus().Publish(rawResp); err != nil {
		return fmt.Errorf("%w: %s", ErrTransport, err)
	}

	return nil
}
