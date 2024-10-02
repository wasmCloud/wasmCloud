//go:generate go run go.wasmcloud.dev/wadge/cmd/wadge-bindgen-go

package main

import (
	"bytes"
	"io"
	"log"
	"log/slog"
	"net/http"
	"os"
	"testing"

	incominghandler "go.wasmcloud.dev/component/gen/wasi/http/incoming-handler"
	"go.wasmcloud.dev/wadge"
	"go.wasmcloud.dev/wadge/wadgehttp"
)

func init() {
	log.SetFlags(0)
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelDebug, ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}

func TestIncomingHandler(t *testing.T) {
	wadge.RunTest(t, func() {
		req, err := http.NewRequest("", "/", nil)
		if err != nil {
			t.Fatalf("failed to create new HTTP request: %s", err)
		}
		resp, err := wadgehttp.HandleIncomingRequest(incominghandler.Exports.Handle, req)
		if err != nil {
			t.Fatalf("failed to handle incoming HTTP request: %s", err)
		}
		if want, got := http.StatusOK, resp.StatusCode; want != got {
			t.Fatalf("unexpected status code: want %d, got %d", want, got)
		}
		buf, err := io.ReadAll(resp.Body)
		if err != nil {
			t.Fatalf("failed to read HTTP response body: %s", err)
		}
		defer resp.Body.Close()

		if want, got := []byte("Hello from Go!\n"), buf; !bytes.Equal(want, got) {
			t.Fatalf("unexpected response body: want %q, got %q", want, got)
		}
	})
}
