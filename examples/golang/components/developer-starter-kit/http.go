package main

import (
	"fmt"
	"io"
	"net/http"
	"strings"

	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasi/keyvalue/atomics"
	"github.com/wasmCloud/wasmcloud/examples/golang/components/developer-starter-kit/gen/wasi/keyvalue/store"
	"go.wasmcloud.dev/component/log/wasilog"
	"go.wasmcloud.dev/component/net/wasihttp"
)

func handleHTTP(w http.ResponseWriter, r *http.Request) {
	switch r.URL.Path {
	case "/", "/hello":
		helloHandler(w, r)
	case "/counter":
		counterHandler(w, r)
	case "/example_dot_com":
		proxyHandler(w, r)
	case "/echo":
		echoHandler(w, r)
	case "/file":
		fileHandler(w, r)
	default:
		http.NotFound(w, r)
	}
}

func helloHandler(w http.ResponseWriter, _ *http.Request) {
	w.Write([]byte("Hello from Starter Kit!\n"))
}

func counterHandler(w http.ResponseWriter, _ *http.Request) {
	maybeBucket := store.Open("default")

	// NOTE: This is a good strategy to handle errors with variants.
	// You can decide which errors to refine and how to handle them,
	// providing a `default` as catch-all. Here we cover all cases as example.
	if maybeBucket.IsErr() {
		err := maybeBucket.Err()
		switch {
		case err.AccessDenied():
			http.Error(w, "Access Denied", http.StatusInternalServerError)
		case err.NoSuchStore():
			http.Error(w, "No Such Store", http.StatusInternalServerError)
		case err.Other() != nil:
			// NOTE: It's good practice to not surface internal errors to users.
			// We do it here to show how to handle variants with an inner string.
			other := err.Other()
			http.Error(w, *other, http.StatusInternalServerError)
		default:
			http.Error(w, "Unknown Error", http.StatusInternalServerError)
		}
		return
	}

	bucket := maybeBucket.OK()

	incRes := atomics.Increment(*bucket, "counter", 2)
	if incRes.IsErr() {
		http.Error(w, "Error incrementing counter", http.StatusInternalServerError)
		return
	}

	currentValue := incRes.OK()

	fmt.Fprintf(w, "Counter: %d\n", *currentValue)
}

func proxyHandler(w http.ResponseWriter, _ *http.Request) {
	logger := wasilog.ContextLogger("proxyHandler")
	// NOTE: Use the wasi default http.Client.
	// You can also build your own `http.Client` using `wasihttp.DefaultTransport` or constructing a `wasihttp.Transport`.
	// var httpClient := &http.Client{Transport: wasihttp.DefaultTransport}
	// var httpClient := &http.Client{Transport: &wasihttp.Transport{ConnectTimeout: 30 * time.Second}}
	httpClient := wasihttp.DefaultClient

	req, err := http.NewRequest(http.MethodGet, "https://example.com", nil)
	if err != nil {
		http.Error(w, "failed to create request", http.StatusBadGateway)
		return
	}

	logger.Info("Sending request", "url", req.URL.String())
	resp, err := httpClient.Do(req)
	if err != nil {
		http.Error(w, "failed to make outbound request", http.StatusBadGateway)
		logger.Error("Failed to make outbound request", "error", err)
		return
	}
	if resp.StatusCode != http.StatusOK {
		http.Error(w, "unexpected status code", http.StatusBadGateway)
		logger.Error("Unexpected status code", "status", resp.StatusCode)
		return
	}

	w.Header().Set("X-Custom-Header", "proxied")

	if _, err := io.Copy(w, resp.Body); err != nil {
		logger.Error("Failed to proxy body", "error", err)
	}
}

func echoHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	if err := r.ParseForm(); err != nil {
		http.Error(w, "Couldn't parse request", http.StatusBadRequest)
		return
	}

	for key, values := range r.Form {
		_, _ = w.Write([]byte(key + ": " + strings.Join(values, ",") + "\n"))
	}

}

func fileHandler(w http.ResponseWriter, r *http.Request) {
	logger := wasilog.ContextLogger("postHandler")

	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	w.WriteHeader(http.StatusOK)

	n, err := io.Copy(w, r.Body)
	if err != nil {
		logger.Error("Error copying body", "error", err)
		return
	}

	logger.Info("Copied body", "bytes", n)
}
