package main

import (
	"bytes"
	"testing"

	"github.com/bytecodealliance/wasm-tools-go/cm"
	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasmcloud/messaging/types"
	wadge "go.wasmcloud.dev/wadge"
)

// NOTE(lxf): this is overridden in tests
func TestMessagingHandle(t *testing.T) {
	previousAdapter := messagingConsumer
	t.Cleanup(func() {
		messagingConsumer = previousAdapter
	})

	t.Run("happyPath", func(t *testing.T) {
		wadge.RunTest(t, func() {
			body := []byte("pls test your components")
			publishCalled := false

			messagingConsumer = &messagingConsumerAdapter{
				Publish: func(msg types.BrokerMessage) (result cm.Result[string, struct{}, string]) {
					publishCalled = true
					if want, got := body, msg.Body.Slice(); !bytes.Equal(want, got) {
						t.Errorf("unexpected body: want %q, got %q", want, got)
					}

					return cm.OK[cm.Result[string, struct{}, string]](struct{}{})
				},
			}

			msg := types.BrokerMessage{
				Body:    cm.ToList(body),
				Subject: "test",
				ReplyTo: cm.Some("reply"),
			}

			res := handleMessage(msg)
			if res.IsErr() {
				t.Errorf("unexpected error: %s", *res.Err())
			}

			if !publishCalled {
				t.Error("expected publish to be called")
			}
		})
	})

	t.Run("publishFailure", func(t *testing.T) {
		wadge.RunTest(t, func() {
			body := []byte("pls test your components")
			publishCalled := false

			messagingConsumer = &messagingConsumerAdapter{
				Publish: func(msg types.BrokerMessage) (result cm.Result[string, struct{}, string]) {
					publishCalled = true
					if want, got := body, msg.Body.Slice(); !bytes.Equal(want, got) {
						t.Errorf("unexpected body: want %q, got %q", want, got)
					}

					return cm.Err[cm.Result[string, struct{}, string]]("boom")
				},
			}

			msg := types.BrokerMessage{
				Body:    cm.ToList(body),
				Subject: "test",
				ReplyTo: cm.Some("reply"),
			}

			res := handleMessage(msg)
			if !res.IsErr() {
				t.Errorf("expected error, didnt happen")
			}

			if !publishCalled {
				t.Error("expected publish to be called")
			}
		})
	})
}
