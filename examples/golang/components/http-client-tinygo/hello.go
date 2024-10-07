package main

import (
	"log"
	"math"
	"net/http"

	"github.com/bytecodealliance/wasm-tools-go/cm"
	incominghandler "github.com/wasmcloud/wasmcloud/examples/golang/components/http-client-tinygo/gen/wasi/http/incoming-handler"
	outgoinghandler "github.com/wasmcloud/wasmcloud/examples/golang/components/http-client-tinygo/gen/wasi/http/outgoing-handler"
	"github.com/wasmcloud/wasmcloud/examples/golang/components/http-client-tinygo/gen/wasi/http/types"
	"github.com/wasmcloud/wasmcloud/examples/golang/components/http-client-tinygo/gen/wasi/io/streams"
)

func init() {
	incominghandler.Exports.Handle = handleRequest
}

func handleRequest(request types.IncomingRequest, responseWriter types.ResponseOutparam) {
	req := types.NewOutgoingRequest(types.NewFields())
	req.SetScheme(cm.Some(types.SchemeHTTPS()))
	req.SetAuthority(cm.Some("dog.ceo"))
	req.SetPathWithQuery(cm.Some("/api/breeds/image/random"))
	result := outgoinghandler.Handle(req, cm.None[types.RequestOptions]())
	if result.IsErr() {
		types.ResponseOutparamSet(responseWriter, cm.Err[cm.Result[types.ErrorCodeShape, types.OutgoingResponse, types.ErrorCode]](
			types.ErrorCodeInternalError(cm.Some("outgoing-handler failed")),
		))
		return
	}

	incomingResponse := result.OK()
	incomingResponse.Subscribe().Block()
	maybeResponse := incomingResponse.Get()
	resp := maybeResponse.Some().OK().OK()
	if resp.Status() != http.StatusOK {
		types.ResponseOutparamSet(responseWriter, cm.Err[cm.Result[types.ErrorCodeShape, types.OutgoingResponse, types.ErrorCode]](
			types.ErrorCodeInternalError(cm.Some("outgoing-handler got non 200 response")),
		))
		return
	}
	maybeResponseBody := resp.Consume()
	responseBody := maybeResponseBody.OK()
	maybeResponseStream := responseBody.Stream()
	responseStream := maybeResponseStream.OK()

	outHeaders := types.NewFields()
	outResponse := types.NewOutgoingResponse(outHeaders)
	outResponse.SetStatusCode(http.StatusOK)
	maybeOutBody := outResponse.Body()
	outBody := maybeOutBody.OK()
	maybeBodyWrite := outBody.Write()
	bodyWrite := maybeBodyWrite.OK()

	okResponse := cm.OK[cm.Result[types.ErrorCodeShape, types.OutgoingResponse, types.ErrorCode]](outResponse)
	types.ResponseOutparamSet(responseWriter, okResponse)

	splice(responseStream, bodyWrite)
	bodyWrite.ResourceDrop()

	types.OutgoingBodyFinish(*outBody, cm.None[types.Fields]())
}

func splice(input *streams.InputStream, output *streams.OutputStream) {
	for {
		res := output.BlockingSplice(*input, math.MaxInt64)
		if res.IsErr() {
			err := res.Err()
			if err.Closed() {
				break
			} else if opFailed := err.LastOperationFailed(); opFailed != nil {
				log.Fatal("last operation failed")
				opFailed.ResourceDrop()
			}
		}
	}
}

//go:generate go run github.com/bytecodealliance/wasm-tools-go/cmd/wit-bindgen-go generate --world hello --out gen ./wit
func main() {}
