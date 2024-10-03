package main

import (
	"bytes"
	"crypto/rand"
	"io"
	"net/http"
	"testing"

	incominghandler "go.wasmcloud.dev/component/gen/wasi/http/incoming-handler"
	"go.wasmcloud.dev/wadge"
	"go.wasmcloud.dev/wadge/wadgehttp"
)

func wadgeRoundtrip(t *testing.T, req *http.Request, f func(*http.Response, error)) {
	t.Helper()

	wadge.RunTest(t, func() {
		f(wadgehttp.HandleIncomingRequest(incominghandler.Exports.Handle, req))
	})
}

func mustNewRequest(t *testing.T, method, url string, body io.Reader) *http.Request {
	t.Helper()

	req, err := http.NewRequest(method, url, body)
	if err != nil {
		t.Fatalf("failed to create new HTTP request: %s", err)
	}
	return req
}

func responseBody(t *testing.T, resp *http.Response) []byte {
	t.Helper()

	if want, got := http.StatusOK, resp.StatusCode; want != got {
		t.Fatalf("unexpected status code: want %d, got %d", want, got)
	}
	buf, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("failed to read HTTP response body: %s", err)
	}

	return buf
}

func TestHTTPHelloHandler(t *testing.T) {
	req := mustNewRequest(t, "GET", "/", nil)
	wadgeRoundtrip(t, req, func(resp *http.Response, respErr error) {
		if respErr != nil {
			t.Fatalf("unexpected error: %s", respErr)
		}
		defer resp.Body.Close()

		buf := responseBody(t, resp)
		if want, got := []byte("Hello from Starter Kit!\n"), buf; !bytes.Equal(want, got) {
			t.Fatalf("unexpected response body: want %q, got %q", want, got)
		}
	})
}

func TestHTTPEchoHandler(t *testing.T) {
	body := bytes.NewBufferString("hello=world")
	req := mustNewRequest(t, "POST", "/echo", body)
	wadgeRoundtrip(t, req, func(resp *http.Response, respErr error) {
		if respErr != nil {
			t.Fatalf("unexpected error: %s", respErr)
		}
		defer resp.Body.Close()

		buf := responseBody(t, resp)
		if want, got := body.Bytes(), buf; !bytes.Equal(want, got) {
			t.Fatalf("unexpected response body: want %q, got %q", want, got)
		}
	})
}

func TestFileHandler(t *testing.T) {
	bigBody := make([]byte, 1024)
	if _, err := rand.Read(bigBody); err != nil {
		t.Fatal(err)
	}
	body := bytes.NewBuffer(bigBody)
	req := mustNewRequest(t, "POST", "/file", body)
	wadgeRoundtrip(t, req, func(resp *http.Response, respErr error) {
		if respErr != nil {
			t.Fatalf("unexpected error: %s", respErr)
		}
		defer resp.Body.Close()

		buf := responseBody(t, resp)
		if want, got := bigBody, buf; !bytes.Equal(want, got) {
			t.Fatalf("unexpected response body: want %q, got %q", want, got)
		}
	})
}

func TestHTTPCounterHandler(t *testing.T) {
	previousStoreName := storeName
	t.Cleanup(func() {
		storeName = previousStoreName
	})
	// NOTE(lxf): wasmtime keyvalue implementation expects "" as valid store name
	storeName = ""

	req := mustNewRequest(t, "GET", "/counter", nil)
	wadgeRoundtrip(t, req, func(resp *http.Response, respErr error) {
		if respErr != nil {
			t.Fatalf("unexpected error: %s", respErr)
		}
		defer resp.Body.Close()

		buf := responseBody(t, resp)
		if want, got := []byte("Counter: 2\n"), buf; !bytes.Equal(want, got) {
			t.Fatalf("unexpected response body: want %q, got %q", want, got)
		}
	})
}

func TestHTTPNotFoundHandler(t *testing.T) {
	req := mustNewRequest(t, "GET", "/some-path-that-doesnt-exist", nil)
	wadgeRoundtrip(t, req, func(resp *http.Response, respErr error) {
		if respErr != nil {
			t.Fatalf("unexpected error: %s", respErr)
		}
		defer resp.Body.Close()

		if want, got := http.StatusNotFound, resp.StatusCode; want != got {
			t.Fatalf("unexpected status code: want %d, got %d", want, got)
		}
	})
}
