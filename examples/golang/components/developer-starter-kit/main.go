package main

import (
	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasmcloud/messaging/handler"
	"go.wasmcloud.dev/component/net/wasihttp"
)

func init() {
	wasihttp.HandleFunc(handleHTTP)
	handler.Exports.HandleMessage = handleMessage
}

//go:generate go run github.com/bytecodealliance/wasm-tools-go/cmd/wit-bindgen-go generate --world starter-kit --out gen ./wit
func main() {}
