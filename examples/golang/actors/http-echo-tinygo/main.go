package main

import (
	echo "echo/gen"
	echo_types "echo/gen"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
)

type EchoResponse struct {
	Method      string `json:"method"`
	Path        string `json:"path"`
	QueryString string `json:"queryString,omitempty"`
	Body        string `json:"body,omitempty"`
}

type Echo struct{}

func (g *Echo) Handle(req echo.ExportsWasiHttp0_2_0_rc_2023_12_05_IncomingHandlerIncomingRequest, resp echo.WasiHttp0_2_0_rc_2023_12_05_TypesResponseOutparam) {
	er := new(EchoResponse)

	method := req.Method()
	switch method {
	case echo.WasiHttp0_2_0_rc_2023_12_05_TypesMethodGet():
		er.Method = "GET"
	case echo.WasiHttp0_2_0_rc_2023_12_05_TypesMethodPost():
		er.Method = "POST"
	case echo.WasiHttp0_2_0_rc_2023_12_05_TypesMethodPut():
		er.Method = "PUT"
	case echo.WasiHttp0_2_0_rc_2023_12_05_TypesMethodDelete():
		er.Method = "DELETE"
	case echo.WasiHttp0_2_0_rc_2023_12_05_TypesMethodPatch():
		er.Method = "PATCH"
	case echo.WasiHttp0_2_0_rc_2023_12_05_TypesMethodConnect():
		er.Method = "CONNECT"
	default:
		er.Method = "OTHER"
	}

	pathWithQuery := req.PathWithQuery()
	if pathWithQuery.IsNone() {
		return
	}

	splitPathQuery := strings.Split(pathWithQuery.Unwrap(), "?")
	er.Path = splitPathQuery[0]
	if len(splitPathQuery) > 1 {
		er.QueryString = splitPathQuery[1]
	}

	maybeBody := req.Consume()
	fields := echo.StaticFieldsFromList([]echo.WasiHttp0_2_0_rc_2023_12_05_TypesTuple2FieldKeyFieldValueT{{F0: "Content-Type", F1: []byte("application/json")}}).Unwrap()
	if maybeBody.IsErr() {
		writeHttpResponse(resp, http.StatusInternalServerError, fields, []byte("{\"error\":\"failed to read request body\"}"))
		return
	}
	body := maybeBody.Unwrap()

	maybeBodyStream := body.Stream()
	if maybeBodyStream.IsErr() {
		writeHttpResponse(resp, http.StatusInternalServerError, fields, []byte("{\"error\":\"failed to convert body into stream\"}"))
		return
	}
	bodyStream := maybeBodyStream.Unwrap()

	maybeReadStream := bodyStream.Read(18446744073709551614)
	if maybeReadStream.IsErr() {
		// If the body is empty, we'll get a closed error, in which case we *do not* want to throw an error.
		errKind := maybeReadStream.UnwrapErr().Kind()
		if errKind == echo.WasiIo0_2_0_rc_2023_11_10_StreamsStreamErrorKindClosed {
			// There was likely *no* data in the body (ex. a GET request)
			er.Body = ""
		} else {
			// if we received some other error, report it
			echo.WasiLoggingLoggingLog(echo.WasiLoggingLoggingLevelError(), "failed to read incoming body stream", fmt.Sprintf("error kind [%v]", ))
			writeHttpResponse(resp, http.StatusInternalServerError, fields, []byte("{\"error\":\"failed to read incoming body stream\"}"))
			return
		}
	} else {
		er.Body = string(maybeReadStream.Unwrap())
	}

	echo.WasiLoggingLoggingLog(echo.WasiLoggingLoggingLevelDebug(), "method", er.Method)
	echo.WasiLoggingLoggingLog(echo.WasiLoggingLoggingLevelDebug(), "path", er.Path)
	echo.WasiLoggingLoggingLog(echo.WasiLoggingLoggingLevelDebug(), "queryString", er.QueryString)
	echo.WasiLoggingLoggingLog(echo.WasiLoggingLoggingLevelDebug(), "body", er.Body)

	bBody, err := json.Marshal(er)
	if err != nil {
		writeHttpResponse(resp, http.StatusInternalServerError, fields, []byte("{\"error\":\"failed to marshal response\"}"))
		return
	}

	writeHttpResponse(resp, http.StatusOK, fields, bBody)
}

func writeHttpResponse(responseOutparam echo.WasiHttp0_2_0_rc_2023_12_05_TypesResponseOutparam, statusCode uint16, headers echo.WasiHttp0_2_0_rc_2023_12_05_TypesHeaders, body []byte) {
	echo.WasiLoggingLoggingLog(echo.WasiLoggingLoggingLevelDebug(), "writeHttpResponse", "writing response: "+string(body))

	outgoingResponse := echo.NewOutgoingResponse(headers)
	outgoingResponse.SetStatusCode(statusCode)

	maybeOutgoingBody := outgoingResponse.Body()
	if maybeOutgoingBody.IsErr() {
		return
	}
	outgoingBody := maybeOutgoingBody.Unwrap()

	maybeOutgoingStream := outgoingBody.Write()
	if maybeOutgoingStream.IsErr() {
		return
	}
	outgoingStream := maybeOutgoingStream.Unwrap()

  res := outgoingStream.BlockingWriteAndFlush(body)
	if res.IsErr() {
		return
	}

	echo.StaticResponseOutparamSet(responseOutparam, echo_types.Ok[echo_types.WasiHttp0_2_0_rc_2023_12_05_TypesOutgoingResponse, echo_types.WasiHttp0_2_0_rc_2023_12_05_TypesErrorCode](outgoingResponse))
}

func init() {
	mg := new(Echo)
	echo.SetExportsWasiHttp0_2_0_rc_2023_12_05_IncomingHandler(mg)
}

func main() {}
