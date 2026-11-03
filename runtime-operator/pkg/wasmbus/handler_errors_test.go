package wasmbus

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"
)

const handlerReplySubject = "response.inbox"

type handlerTestBus struct {
	Bus
	publish      func(*Message) error
	subscribe    func(string, int) (Subscription, error)
	publishCalls int
	published    *Message
}

func (b *handlerTestBus) Publish(msg *Message) error {
	b.publishCalls++
	b.published = msg
	if b.publish != nil {
		return b.publish(msg)
	}
	return nil
}

func (b *handlerTestBus) Subscribe(subject string, backlog int) (Subscription, error) {
	return b.subscribe(subject, backlog)
}

type handlerTestSubscription struct {
	callback SubscriptionCallback
	drains   int
	err      error
}

func (s *handlerTestSubscription) Handle(callback SubscriptionCallback) { s.callback = callback }
func (s *handlerTestSubscription) Drain() error {
	s.drains++
	return s.err
}

func TestRequestHandlerStopsOnFailure(t *testing.T) {
	const (
		decodeStage  = "decode"
		preStage     = "pre"
		handlerStage = "handler"
		postStage    = "post"
		publishStage = "publish"
	)
	for _, tc := range []struct {
		stage string
		class error
		calls []string
	}{
		{decodeStage, ErrDecode, nil},
		{preStage, ErrOperation, []string{preStage}},
		{handlerStage, ErrOperation, []string{preStage, handlerStage}},
		{postStage, ErrOperation, []string{preStage, handlerStage, postStage}},
		{publishStage, ErrTransport, []string{preStage, handlerStage, postStage, publishStage}},
	} {
		t.Run(tc.stage, func(t *testing.T) {
			cause := errors.New("pipeline failed")
			var calls []string
			step := func(stage string) error {
				calls = append(calls, stage)
				if stage == tc.stage {
					return cause
				}
				return nil
			}
			bus := &handlerTestBus{publish: func(*Message) error { return step(publishStage) }}
			msg := NewMessage(testSubject)
			msg.Reply = handlerReplySubject
			msg.bus = bus
			msg.Data = []byte(`{"name":"request"}`)
			if tc.stage == decodeStage {
				msg.Data = []byte("{")
			}
			handler := NewRequestHandler(testMessage{}, testResponse{},
				func(context.Context, *testMessage) (*testResponse, error) {
					return &testResponse{Hello: "response"}, step(handlerStage)
				})
			handler.PreRequest = func(context.Context, *testMessage, *Message) error { return step(preStage) }
			handler.PostRequest = func(context.Context, *testResponse, *Message) error { return step(postStage) }
			err := handler.HandleMessage(t.Context(), msg)
			if !errors.Is(err, tc.class) {
				t.Fatalf("HandleMessage = %v, want %v", err, tc.class)
			}
			if tc.stage != decodeStage && !strings.Contains(err.Error(), cause.Error()) {
				t.Fatalf("error lost its diagnostic: %v", err)
			}
			if !reflect.DeepEqual(calls, tc.calls) {
				t.Fatalf("pipeline calls = %v, want %v", calls, tc.calls)
			}
			wantPublishes := 0
			if tc.stage == publishStage {
				wantPublishes = 1
			}
			if bus.publishCalls != wantPublishes {
				t.Fatalf("publish attempts = %d, want %d", bus.publishCalls, wantPublishes)
			}
		})
	}
}

func TestRequestHandlerEncodingFailureDoesNotPublish(t *testing.T) {
	bus := &handlerTestBus{}
	msg := NewMessage(testSubject)
	msg.bus = bus
	handler := NewRequestHandler(testRequest{}, testMessageErr{},
		func(context.Context, *testRequest) (*testMessageErr, error) { return &testMessageErr{}, nil })
	handler.PostRequest = func(context.Context, *testMessageErr, *Message) error {
		t.Fatal("post hook ran after encoding failed")
		return nil
	}
	if err := handler.HandleMessage(t.Context(), msg); !errors.Is(err, ErrEncode) {
		t.Fatalf("HandleMessage = %v, want ErrEncode", err)
	}
	if bus.publishCalls != 0 {
		t.Fatal("encoding failure published a response")
	}
}

func TestRequestHandlerHooksPreserveValueTemplate(t *testing.T) {
	type contextKey struct{}
	ctx := context.WithValue(t.Context(), contextKey{}, requestName)
	bus := &handlerTestBus{}
	template := testMessage{Name: "default", NotDecoded: "template"}
	handler := NewRequestHandler(template, testResponse{},
		func(ctx context.Context, req *testMessage) (*testResponse, error) {
			if ctx.Value(contextKey{}) != requestName || req.NotDecoded != template.NotDecoded {
				t.Fatalf("handler context or request defaults lost: %+v", req)
			}
			req.NotDecoded = "changed"
			return &testResponse{Hello: req.Name}, nil
		})
	handler.PreRequest = func(ctx context.Context, req *testMessage, msg *Message) error {
		if ctx.Value(contextKey{}) != requestName || msg.Bus() != bus {
			t.Fatal("pre hook lost the request context or bus")
		}
		req.Name += "-prepared"
		return nil
	}
	handler.PostRequest = func(ctx context.Context, resp *testResponse, msg *Message) error {
		if ctx.Value(contextKey{}) != requestName || msg.Bus() != bus || resp.Hello == "" {
			t.Fatal("post hook lost the response context or bus")
		}
		msg.Header.Set("X-Processed", "true")
		return nil
	}
	for _, data := range []string{`{"name":"request"}`, ""} {
		msg := NewMessage(testSubject)
		msg.Reply = handlerReplySubject
		msg.Data = []byte(data)
		msg.bus = bus
		if err := handler.HandleMessage(ctx, msg); err != nil {
			t.Fatal(err)
		}
		want := `{"hello":"request-prepared"}`
		if data == "" {
			want = `{"hello":"default-prepared"}`
		}
		reply := bus.published
		if reply == nil || reply.Subject != msg.Reply || string(reply.Data) != want {
			t.Fatalf("reply = %+v, want %q on %q", reply, want, msg.Reply)
		}
		if reply.Header.Get("Content-Type") != mimetypeJSON || reply.Header.Get("X-Processed") != "true" {
			t.Fatalf("response headers = %v", reply.Header)
		}
	}
	if handler.Request != template || bus.publishCalls != 2 {
		t.Fatalf("request template or publish count changed: %+v, %d", handler.Request, bus.publishCalls)
	}
}

func TestRequestHandlerAllocatesReferenceFieldsPerRequest(t *testing.T) {
	type request struct {
		Labels map[string]string `json:"labels"`
		Values []string          `json:"values"`
		Nested *testMessage      `json:"nested"`
	}
	const (
		secondName = "second"
		changed    = "mutated"
	)
	var requests []*request
	handler := NewRequestHandler(request{}, testResponse{},
		func(_ context.Context, req *request) (*testResponse, error) {
			requests = append(requests, req)
			return &testResponse{}, nil
		})
	bus := &handlerTestBus{}
	for _, data := range []string{
		`{"labels":{"name":"first"},"values":["first"],"nested":{"name":"first"}}`,
		`{"labels":{"name":"second"},"values":["second"],"nested":{"name":"second"}}`,
		`{}`,
	} {
		msg := NewMessage(testSubject)
		msg.Reply = handlerReplySubject
		msg.Data = []byte(data)
		msg.bus = bus
		if err := handler.HandleMessage(t.Context(), msg); err != nil {
			t.Fatal(err)
		}
	}
	first, second, empty := requests[0], requests[1], requests[2]
	for _, req := range requests[:2] {
		if len(req.Labels) != 1 || len(req.Values) != 1 || req.Nested == nil {
			t.Fatalf("decoded reference fields = %+v", req)
		}
	}
	first.Labels["name"] = changed
	first.Values[0] = changed
	first.Nested.Name = changed
	if second.Labels["name"] != secondName || second.Values[0] != secondName || second.Nested.Name != secondName {
		t.Fatalf("requests share decoded storage: %+v", second)
	}
	for _, req := range []*request{empty, &handler.Request} {
		if req.Labels != nil || req.Values != nil || req.Nested != nil {
			t.Fatalf("empty request or value template retained decoded storage: %+v", req)
		}
	}
}

func TestServerRegisterHandlerSubscriptionFailure(t *testing.T) {
	want := errors.New("subscription failed")
	bus := &handlerTestBus{subscribe: func(subject string, backlog int) (Subscription, error) {
		if subject != testSubject || backlog != NoBackLog {
			t.Fatalf("Subscribe(%q, %d)", subject, backlog)
		}
		return nil, want
	}}
	s := NewServer(bus, "test")
	t.Cleanup(func() { _ = s.Drain() })
	err := s.RegisterHandler(testSubject, ServerHandlerFunc(func(context.Context, *Message) error {
		t.Fatal("handler ran after subscription failed")
		return nil
	}))
	if !errors.Is(err, want) || len(s.subscriptions) != 0 {
		t.Fatalf("RegisterHandler = %v, subscriptions = %d", err, len(s.subscriptions))
	}
}

func TestServerDrainJoinsSubscriptionErrors(t *testing.T) {
	first := errors.New("first drain failed")
	last := errors.New("last drain failed")
	subs := []*handlerTestSubscription{{err: first}, {}, {err: last}}
	s := NewServer(nil, "test")
	for _, sub := range subs {
		s.subscriptions = append(s.subscriptions, sub)
	}
	err := s.Drain()
	if !errors.Is(err, first) || !errors.Is(err, last) {
		t.Fatalf("Drain = %v, want both failures", err)
	}
	for i, sub := range subs {
		if sub.drains != 1 {
			t.Fatalf("subscription %d drained %d times, want once", i, sub.drains)
		}
	}
	if len(s.subscriptions) != 0 {
		t.Fatal("drained subscriptions retained")
	}
	select {
	case _, ok := <-s.ErrorStream():
		if ok {
			t.Fatal("error stream is still open")
		}
	default:
		t.Fatal("error stream was not closed")
	}
}

func TestServerErrorsRetainRequestContext(t *testing.T) {
	sub := &handlerTestSubscription{}
	bus := &handlerTestBus{subscribe: func(string, int) (Subscription, error) { return sub, nil }}
	s := NewServer(bus, "test")
	s.errorStream = make(chan *ServerError, 1)
	t.Cleanup(func() { _ = s.Drain() })
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	s.ContextFunc = func() context.Context { return ctx }
	want := errors.New("request handler failed")
	handler := ServerHandlerFunc(func(context.Context, *Message) error { return want })
	if err := s.RegisterHandler(testSubject, handler); err != nil {
		t.Fatal(err)
	}
	msg := NewMessage(testSubject)
	sub.callback(msg)
	select {
	case got := <-s.ErrorStream():
		if got.Request != msg || got.Context != ctx || !errors.Is(got.Err, want) {
			t.Fatalf("reported server error = %+v", got)
		}
	default:
		t.Fatal("handler error was not reported")
	}
}
