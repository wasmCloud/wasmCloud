package main

import (
	"net/http"

	"github.com/bytecodealliance/wasm-tools-go/cm"
	incominghandler "github.com/wasmcloud/wasmcloud/examples/golang/components/http-hello-world/gen/wasi/http/incoming-handler"
	"github.com/wasmcloud/wasmcloud/examples/golang/components/http-hello-world/gen/wasi/http/types"
	"github.com/wasmcloud/wasmcloud/examples/golang/components/http-hello-world/gen/wasi/io/streams"
)

func init() {
	incominghandler.Exports.Handle = handleRequest
}

func handleRequest(request types.IncomingRequest, responseWriter types.ResponseOutparam) {
	headers := types.NewFields()
	httpResponse := types.NewOutgoingResponse(headers)
	httpResponse.SetStatusCode(http.StatusOK)

	var body *types.OutgoingBody
	var bodyStream *streams.OutputStream
	if bodyResource := httpResponse.Body(); bodyResource.IsErr() {
		types.ResponseOutparamSet(responseWriter, cm.Err[cm.Result[types.ErrorCodeShape, types.OutgoingResponse, types.ErrorCode]](
			types.ErrorCodeInternalError(cm.Some("couldn't create body resource")),
		))
		return
	} else {
		body = bodyResource.OK()
	}
	if bodyStreamResource := body.Write(); bodyStreamResource.IsErr() {
		types.ResponseOutparamSet(responseWriter, cm.Err[cm.Result[types.ErrorCodeShape, types.OutgoingResponse, types.ErrorCode]](
			types.ErrorCodeInternalError(cm.Some("couldn't create body stream")),
		))
		return
	} else {
		bodyStream = bodyStreamResource.OK()
	}

	// Prepare the response by writing status & headers
	okResponse := cm.OK[cm.Result[types.ErrorCodeShape, types.OutgoingResponse, types.ErrorCode]](httpResponse)
	types.ResponseOutparamSet(responseWriter, okResponse)

	// Write the body
	bodyStream.BlockingWriteAndFlush(cm.ToList([]uint8("Hello from Go!\n")))
	// Release the body stream ( nested resource )
	bodyStream.ResourceDrop()

	// Finish the body, releasing the body resource
	// The second argument is for HTTP Trailers, usefull for HTTP/2
	types.OutgoingBodyFinish(*body, cm.None[types.Fields]())
}

//go:generate go run github.com/bytecodealliance/wasm-tools-go/cmd/wit-bindgen-go generate --world hello --out gen ./wit
func main() {}
