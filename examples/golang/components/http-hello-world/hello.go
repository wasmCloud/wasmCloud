package main

import (
	http "github.com/wasmcloud/wasmcloud/examples/golang/actors/http-hello-world/gen"
)

// Helper type aliases to make code more readable
type HttpRequest = http.ExportsWasiHttp0_2_0_IncomingHandlerIncomingRequest
type HttpResponseWriter = http.ExportsWasiHttp0_2_0_IncomingHandlerResponseOutparam
type HttpOutgoingResponse = http.WasiHttp0_2_0_TypesOutgoingResponse
type HttpError = http.WasiHttp0_2_0_TypesErrorCode

type HttpServer struct{}

func init() {
	httpserver := HttpServer{}
	// Set the incoming handler struct to HttpServer
	http.SetExportsWasiHttp0_2_0_IncomingHandler(httpserver)
}

func (h HttpServer) Handle(request HttpRequest, responseWriter HttpResponseWriter) {
	// Construct HttpResponse to send back
	headers := http.NewFields()
	httpResponse := http.NewOutgoingResponse(headers)
	httpResponse.SetStatusCode(200)
	httpResponse.Body().Unwrap().Write().Unwrap().BlockingWriteAndFlush([]uint8("Hello from Go!\n")).Unwrap()

	// Send HTTP response
	okResponse := http.Ok[HttpOutgoingResponse, HttpError](httpResponse)
	http.StaticResponseOutparamSet(responseWriter, okResponse)
}

//go:generate wit-bindgen tiny-go wit --out-dir=gen --gofmt
func main() {}
