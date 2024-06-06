package main

import (
	"log"
	"math"

	http "github.com/wasmcloud/wasmcloud/examples/golang/components/http-client-tinygo/gen"
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
	req := http.NewOutgoingRequest(http.NewFields())
	req.SetScheme(http.Some[http.WasiHttp0_2_0_TypesScheme](http.WasiHttp0_2_0_TypesSchemeHttps()))
	req.SetAuthority(http.Some("dog.ceo"))
	req.SetPathWithQuery(http.Some("/api/breeds/image/random"))
	result := http.WasiHttp0_2_0_OutgoingHandlerHandle(req, http.None[http.WasiHttp0_2_0_OutgoingHandlerRequestOptions]())
	if result.IsOk() {
		incomingResponse := result.Unwrap()
		incomingResponse.Subscribe().Block()
		resp := incomingResponse.Get().Unwrap().Unwrap().Unwrap()
		if resp.Status() == 200 {
			responseBody := resp.Consume().Unwrap()
			headers := http.NewFields()
			httpResponse := http.NewOutgoingResponse(headers)
			httpResponse.SetStatusCode(200)
			body := httpResponse.Body().Unwrap()
			bodyWrite := body.Write().Unwrap()
			instream := responseBody.Stream().Unwrap()
			okResponse := http.Ok[HttpOutgoingResponse, HttpError](httpResponse)
			http.StaticResponseOutparamSet(responseWriter, okResponse)
			splice(instream, bodyWrite)
			bodyWrite.Drop()
			http.StaticOutgoingBodyFinish(body, http.None[http.WasiHttp0_2_0_TypesTrailers]())
		}

	} else {
		errorCode := result.UnwrapErr()
		log.Fatal("Error fetching response from http provider", errorCode)
	}
}

func splice(input http.WasiIo0_2_0_StreamsInputStream, output http.WasiIo0_2_0_StreamsOutputStream) {
	for {
		res := output.BlockingSplice(input, math.MaxUint64)
		if res.IsErr() {
			err := res.UnwrapErr()
			if err.Kind() == http.WasiIo0_2_0_StreamsStreamErrorKindClosed {
				break
			} else if err.Kind() == http.WasiIo0_2_0_StreamsStreamErrorKindLastOperationFailed {
				log.Fatal("last operation failed")
			}
		}
	}
}

//go:generate wit-bindgen tiny-go wit --out-dir=gen --gofmt
func main() {}
