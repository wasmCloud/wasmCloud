//go:generate go tool wit-bindgen-go generate --world hello --out gen ./wit

package main

import (
	"fmt"
	"io"
	"net/http"

	"go.wasmcloud.dev/component/log/wasilog"
	"go.wasmcloud.dev/component/net/wasihttp"
)

var (
	wasiTransport = &wasihttp.Transport{}
	httpClient    = &http.Client{Transport: wasiTransport}
)

func init() {
	wasihttp.HandleFunc(handler)
}

func handler(w http.ResponseWriter, r *http.Request) {
	logger := wasilog.ContextLogger("handler")

	url := "https://dog.ceo/api/breeds/image/random"
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		logger.Error("failed to create outbound request", "err", err)
		http.Error(w, fmt.Sprintf("handler: failed to create outbound request: %s", err), http.StatusInternalServerError)
		return
	}

	resp, err := httpClient.Do(req)
	if err != nil {
		logger.Error("failed to make outbound request", "err", err)
		http.Error(w, fmt.Sprintf("handler: failed to make outbound request: %s", err), http.StatusInternalServerError)
		return
	}

	w.WriteHeader(resp.StatusCode)
	_, _ = io.Copy(w, resp.Body)
}

func main() {}
