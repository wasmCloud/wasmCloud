package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"strings"

	echo "github.com/wasmcloud/wasmcloud/examples/golang/components/http-echo-tinygo/gen"
)

// A struct containing information about a request,
// sent back as a response JSON from the echo server
type EchoResponse struct {
	Method      string `json:"method"`
	Path        string `json:"path"`
	QueryString string `json:"queryString,omitempty"`
	Body        string `json:"body,omitempty"`
}

// Implmenetation struct for the `echo` world (see `wit/echo.wit`)
type Echo struct{}

type HttpRequest = echo.ExportsWasiHttp0_2_0_IncomingHandlerIncomingRequest
type HttpResponseWriter = echo.WasiHttp0_2_0_TypesResponseOutparam
type KeyValueTuple = echo.WasiHttp0_2_0_TypesTuple2FieldKeyFieldValueT
type Headers = echo.WasiHttp0_2_0_TypesHeaders

// Implementation of the `wasi-http:incoming-handler` export in the `echo` world (see `wit/echo.wit)`
//
// This method's signature and implementation use code generated by `wit-bindgen`, in the `gen` folder
// When building with `wash build`, `wit-bindgen` is run automatically to generate the classes that have been aliased above.
func (g *Echo) Handle(req HttpRequest, resp HttpResponseWriter) {
	er := new(EchoResponse)

	// Detect request method, use to build EchoResponse
	method := req.Method()
	switch method {
	case echo.WasiHttp0_2_0_TypesMethodGet():
		er.Method = "GET"
	case echo.WasiHttp0_2_0_TypesMethodPost():
		er.Method = "POST"
	case echo.WasiHttp0_2_0_TypesMethodPut():
		er.Method = "PUT"
	case echo.WasiHttp0_2_0_TypesMethodDelete():
		er.Method = "DELETE"
	case echo.WasiHttp0_2_0_TypesMethodPatch():
		er.Method = "PATCH"
	case echo.WasiHttp0_2_0_TypesMethodConnect():
		er.Method = "CONNECT"
	default:
		er.Method = "OTHER"
	}

	// Retrieve the request path w/ query fragment (an echo.Option[T])
	pathWithQuery := req.PathWithQuery()
	if pathWithQuery.IsNone() {
		return
	}

	// Split the path to retrieve the query element, building the EchoResponse object
	splitPathQuery := strings.Split(pathWithQuery.Unwrap(), "?")
	er.Path = splitPathQuery[0]
	if len(splitPathQuery) > 1 {
		er.QueryString = splitPathQuery[1]
	}

	// Create a list of keyvalue tuples usable as response headers
	// NOTE: this is a manual append because of templating
	var header_tuples []KeyValueTuple
	header_tuples = append(header_tuples, KeyValueTuple{F0: "Content-Type", F1: []byte("application/json")})
	resp_headers := echo.StaticFieldsFromList(header_tuples).Unwrap()

	// Consume the request body
	maybeBody := req.Consume()
	if maybeBody.IsErr() {
		writeHttpResponse(resp, http.StatusInternalServerError, resp_headers, []byte("{\"error\":\"failed to read request body\"}"))
		return
	}
	body := maybeBody.Unwrap()

	// Convert the request body to a stream
	maybeBodyStream := body.Stream()
	if maybeBodyStream.IsErr() {
		writeHttpResponse(resp, http.StatusInternalServerError, resp_headers, []byte("{\"error\":\"failed to convert body into stream\"}"))
		return
	}
	bodyStream := maybeBodyStream.Unwrap()

	// Read the maximum amount of bytes possible from the stream
	maybeReadStream := bodyStream.Read(18446744073709551614)
	if maybeReadStream.IsErr() {
		// If the body is empty, we'll get a closed error, in which case we *do not* want to throw an error.
		errKind := maybeReadStream.UnwrapErr().Kind()
		if errKind == echo.WasiIo0_2_0_StreamsStreamErrorKindClosed {
			// There was likely *no* data in the body (ex. a GET request)
			er.Body = ""
		} else {
			// if we received some other error, report it
			echo.WasiLogging0_1_0_draft_LoggingLog(echo.WasiLogging0_1_0_draft_LoggingLevelError(), "failed to read incoming body stream", fmt.Sprintf("error kind [%v]"))
			writeHttpResponse(resp, http.StatusInternalServerError, resp_headers, []byte("{\"error\":\"failed to read incoming body stream\"}"))
			return
		}
	} else {
		// If reading from the request did not error, we can update the EchoResponse object with the request body
		er.Body = string(maybeReadStream.Unwrap())
	}

	// Log information about the request
	echo.WasiLogging0_1_0_draft_LoggingLog(echo.WasiLogging0_1_0_draft_LoggingLevelDebug(), "method", er.Method)
	echo.WasiLogging0_1_0_draft_LoggingLog(echo.WasiLogging0_1_0_draft_LoggingLevelDebug(), "path", er.Path)
	echo.WasiLogging0_1_0_draft_LoggingLog(echo.WasiLogging0_1_0_draft_LoggingLevelDebug(), "queryString", er.QueryString)
	echo.WasiLogging0_1_0_draft_LoggingLog(echo.WasiLogging0_1_0_draft_LoggingLevelDebug(), "body", er.Body)

	// Marshal the EchoResponse object we've been building to JSON
	bBody, err := json.Marshal(er)
	if err != nil {
		writeHttpResponse(resp, http.StatusInternalServerError, resp_headers, []byte("{\"error\":\"failed to marshal response\"}"))
		return
	}

	writeHttpResponse(resp, http.StatusOK, resp_headers, bBody)
}

// Write an outgoing HTTP response (status, headers, body) to a given writer (in WIT terms a ResponseOutparam)
func writeHttpResponse(responseOutparam HttpResponseWriter, statusCode uint16, headers Headers, body []byte) {
	echo.WasiLogging0_1_0_draft_LoggingLog(echo.WasiLogging0_1_0_draft_LoggingLevelDebug(), "writeHttpResponse", "writing response: "+string(body))

	// Build the new HTTP outgoing response
	outgoingResponse := echo.NewOutgoingResponse(headers)
	outgoingResponse.SetStatusCode(statusCode)

	// Retrieve the body inside the outgoing response
	maybeOutgoingBody := outgoingResponse.Body()
	if maybeOutgoingBody.IsErr() {
		return
	}
	outgoingBody := maybeOutgoingBody.Unwrap()

	// Create a writable stream for the response body content
	maybeOutgoingStream := outgoingBody.Write()
	if maybeOutgoingStream.IsErr() {
		return
	}
	outgoingStream := maybeOutgoingStream.Unwrap()

	// Write the body to the outgoing response
	res := outgoingStream.BlockingWriteAndFlush(body)
	if res.IsErr() {
		return
	}

	outgoingStream.Drop()
	echo.StaticOutgoingBodyFinish(outgoingBody, echo.None[echo.WasiHttp0_2_0_TypesTrailers]())

	// Set the response on the outparam
	echo.StaticResponseOutparamSet(responseOutparam, echo.Ok[echo.WasiHttp0_2_0_TypesOutgoingResponse, echo.WasiHttp0_2_0_TypesErrorCode](outgoingResponse))
}

func init() {
	mg := new(Echo)
	echo.SetExportsWasiHttp0_2_0_IncomingHandler(mg)
}

// NOTE: the below go-generate line is not strictly necessary when using `wash build`,
// but it enables use with the `go` and Bytecode Alliance `wit-bindgen` tooling
//
//go:generate wit-bindgen tiny-go wit --out-dir=gen --gofmt
func main() {}
