package wasmbus

import (
	"context"
	"errors"
	"testing"
	"time"

	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus/wasmbustest"
)

type testMessage struct {
	Name       string `json:"name"`
	NotDecoded string `json:"-"`
}

type testMessageErr struct {
	Name       string `json:"name"`
	NotDecoded string `json:"-"`
}

func (t *testMessageErr) MarshalJSON() ([]byte, error) {
	return nil, errors.New("boom")
}

func (t *testMessageErr) UnmarshalJSON([]byte) error {
	return errors.New("boom")
}

func TestEncode(t *testing.T) {
	req := &testMessage{Name: "test"}
	reqMsg, err := Encode("test", req)
	if err != nil {
		t.Errorf("Encode failed: %s", err)
	}
	if want, got := "test", reqMsg.Subject; want != got {
		t.Errorf("Encode failed: subject: want %s, got %s", want, got)
	}
	if want, got := "application/json", reqMsg.Header.Get("Content-Type"); want != got {
		t.Errorf("Encode failed: content type: want %s, got %s", want, got)
	}
	if want, got := `{"name":"test"}`, string(reqMsg.Data); want != got {
		t.Errorf("Encode failed: data: want %s, got %s", want, got)
	}

	reqMsg, err = Encode("test", &testMessageErr{Name: "test"})
	if err == nil {
		t.Errorf("expected error for nil payload, got '%s'", string(reqMsg.Data))
	}
}

func TestDecode(t *testing.T) {
	t.Run("json", func(t *testing.T) {
		resp := &testMessage{}
		respMsg := &Message{
			Subject: "test",
			Header:  make(Header),
			Data:    []byte(`{"name":"test"}`),
		}
		respMsg.Header.Set("Content-Type", "application/json")
		if err := Decode(respMsg, resp); err != nil {
			t.Errorf("Decode failed: %s", err)
		}
		if want, got := "test", resp.Name; want != got {
			t.Errorf("Decode failed: name: want %s, got %s", want, got)
		}

		resp = &testMessage{}
		respMsg.Data = []byte(`{"name":"test"`)
		if err := Decode(respMsg, resp); err == nil {
			t.Errorf("expected error for invalid json, got '%s'", resp.Name)
		}
	})

	t.Run("yaml", func(t *testing.T) {
		resp := &testMessage{}
		respMsg := &Message{
			Subject: "test",
			Header:  make(Header),
			Data:    []byte(`name: test`),
		}
		respMsg.Header.Set("Content-Type", "application/yaml")
		if err := Decode(respMsg, resp); err != nil {
			t.Errorf("Decode failed: %s", err)
		}

		resp = &testMessage{}
		respMsg.Data = []byte(`:"test"`)
		if err := Decode(respMsg, resp); err == nil {
			t.Errorf("expected error for invalid yaml, got '%s'", resp.Name)
		}
	})

	t.Run("invalid content type", func(t *testing.T) {
		resp := &testMessage{}
		respMsg := &Message{
			Subject: "test",
			Header:  make(Header),
			Data:    []byte(`<name>test</name>`),
		}
		respMsg.Header.Set("Content-Type", "application/xml")
		if err := Decode(respMsg, resp); err == nil {
			t.Errorf("expected error for invalid content type, got '%s'", resp.Name)
		}
	})
}

func TestLatticeRequest(t *testing.T) {
	defer wasmbustest.MustStartNats(t)()
	nc, err := NatsConnect(NatsDefaultURL)
	if err != nil {
		t.Fatal(err)
	}
	defer nc.Close()

	bus := NewNatsBus(nc)
	t.Run("success", func(t *testing.T) {
		sub, err := bus.Subscribe("test", NoBackLog)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = sub.Drain() }()

		errCh := make(chan error, 1)
		sub.Handle(func(msg *Message) {
			resp := &testMessage{}
			if err := Decode(msg, resp); err != nil {
				errCh <- err
				return
			}
			resp.Name = "resp"
			respMsg, err := Encode(msg.Reply, resp)
			if err != nil {
				errCh <- err
				return
			}
			if err := bus.Publish(respMsg); err != nil {
				errCh <- err
				return
			}
		})
		if err := nc.Flush(); err != nil {
			t.Fatal(err)
		}

		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		req := NewLatticeRequest(bus, "test", &testMessage{Name: "request"}, testMessage{})
		resp, err := req.Execute(ctx)
		if err != nil {
			t.Errorf("Execute failed: %s", err)
		}

		select {
		case err := <-errCh:
			t.Fatalf("server error: %s", err)
		default:
		}

		if want, got := "resp", resp.Name; want != got {
			t.Errorf("Execute failed: name: want %s, got %s", want, got)
		}
	})
	t.Run("encode error", func(t *testing.T) {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		req := NewLatticeRequest(bus, "test", &testMessageErr{Name: "request"}, testMessage{})
		_, err := req.Execute(ctx)
		if err == nil {
			t.Errorf("expected error for invalid payload")
		}
	})

	t.Run("decode error", func(t *testing.T) {
		sub, err := bus.Subscribe("test", NoBackLog)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = sub.Drain() }()

		sub.Handle(func(msg *Message) {
			respMsg := NewMessage(msg.Reply)
			respMsg.Header.Set("Content-Type", "bricks")
			respMsg.Data = []byte("boom")
			_ = bus.Publish(respMsg)
		})
		if err := nc.Flush(); err != nil {
			t.Fatal(err)
		}

		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		req := NewLatticeRequest(bus, "test", &testMessage{Name: "request"}, testMessage{})
		_, err = req.Execute(ctx)
		if err == nil {
			t.Errorf("expected error for invalid payload")
		}
	})

	t.Run("timeout error", func(t *testing.T) {
		req := NewLatticeRequest(bus, "test", &testMessage{Name: "request"}, testMessage{})
		ctx, cancel := context.WithTimeout(context.Background(), 1)
		defer cancel()
		_, err = req.Execute(ctx)
		if err == nil {
			t.Errorf("expected error for timeout")
		}
	})

	t.Run("pre-request", func(t *testing.T) {
		sub, err := bus.Subscribe("test", NoBackLog)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = sub.Drain() }()

		errCh := make(chan error, 1)
		sub.Handle(func(msg *Message) {
			resp := &testMessage{}
			if err := Decode(msg, resp); err != nil {
				errCh <- err
				return
			}
			if want, got := "pre-request", resp.Name; want != got {
				errCh <- errors.New("pre-request failed")
			}
			respMsg := NewMessage(msg.Reply)
			_ = bus.Publish(respMsg)
		})
		if err := nc.Flush(); err != nil {
			t.Fatal(err)
		}

		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		req := NewLatticeRequest(bus, "test", &testMessage{Name: "request"}, testMessage{})
		req.PreRequest = func(ctx context.Context, t *testMessage, m *Message) error {
			m.Data = []byte(`{"name":"pre-request"}`)
			return nil
		}
		_, err = req.Execute(ctx)
		if err != nil {
			t.Errorf("Execute failed: %s", err)
		}

		select {
		case err := <-errCh:
			t.Fatalf("server error: %s", err)
		default:
		}

		req.PreRequest = func(ctx context.Context, t *testMessage, m *Message) error {
			return errors.New("boom")
		}
		ctx2, cancel2 := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel2()
		_, err = req.Execute(ctx2)
		if err == nil {
			t.Errorf("expected error for pre-request")
		}
	})

	t.Run("post-request", func(t *testing.T) {
		sub, err := bus.Subscribe("test", NoBackLog)
		if err != nil {
			t.Fatal(err)
		}
		defer func() { _ = sub.Drain() }()

		errCh := make(chan error, 1)
		sub.Handle(func(msg *Message) {
			resp := &testMessage{}
			if err := Decode(msg, resp); err != nil {
				errCh <- err
				return
			}
			resp.Name = "response"
			respMsg, err := Encode(msg.Reply, resp)
			if err != nil {
				errCh <- err
				return
			}
			if err := bus.Publish(respMsg); err != nil {
				errCh <- err
				return
			}
		})
		if err := nc.Flush(); err != nil {
			t.Fatal(err)
		}

		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		req := NewLatticeRequest(bus, "test", &testMessage{Name: "request"}, testMessage{})
		req.PostRequest = func(ctx context.Context, t *testMessage, m *Message) error {
			t.Name = "post-request"
			return nil
		}
		resp, err := req.Execute(ctx)
		if err != nil {
			t.Errorf("Execute failed: %s", err)
		}

		if want, got := "post-request", resp.Name; want != got {
			t.Errorf("Execute failed: name: want %s, got %s", want, got)
		}

		req.PostRequest = func(ctx context.Context, t *testMessage, m *Message) error {
			return errors.New("boom")
		}
		ctx2, cancel2 := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel2()
		_, err = req.Execute(ctx2)
		if err == nil {
			t.Errorf("expected error for post-request")
		}
	})
}
