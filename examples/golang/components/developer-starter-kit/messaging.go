package main

import (
	"github.com/bytecodealliance/wasm-tools-go/cm"
	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasmcloud/messaging/consumer"
	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasmcloud/messaging/types"
	"go.wasmcloud.dev/component/log/wasilog"
)

type messagingConsumerAdapter struct {
	Publish func(msg types.BrokerMessage) (result cm.Result[string, struct{}, string])
}

// NOTE(lxf): this is overridden in tests
var messagingConsumer = &messagingConsumerAdapter{
	Publish: consumer.Publish,
}

func handleMessage(msg types.BrokerMessage) cm.Result[string, struct{}, string] {
	logger := wasilog.ContextLogger("handleMessage")
	replyTo := msg.ReplyTo.Some()

	logger.Info("Received message", "subject", msg.Subject)

	if replyTo != nil {
		logger.Info("Sending reply", "subject", *replyTo)

		reply := types.BrokerMessage{
			Subject: *replyTo,
			Body:    msg.Body,
			ReplyTo: cm.None[string](),
		}
		res := messagingConsumer.Publish(reply)
		if res.IsErr() {
			logger.Error("Failed to send reply", "error", *res.Err())
			return res
		}
	}

	return cm.OK[cm.Result[string, struct{}, string]](struct{}{})
}
