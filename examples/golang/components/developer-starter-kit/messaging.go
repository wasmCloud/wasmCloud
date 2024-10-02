package main

import (
	"github.com/bytecodealliance/wasm-tools-go/cm"
	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasmcloud/messaging/consumer"
	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasmcloud/messaging/types"
	"go.wasmcloud.dev/component/log/wasilog"
)

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
		consumer.Publish(reply)
	}

	return cm.OK[cm.Result[string, struct{}, string]](struct{}{})
}
