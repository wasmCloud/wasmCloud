package main

import (
	"net/http"

	"go.wasmcloud.dev/component/net/wasihttp"
)

func init() {
	wasihttp.HandleFunc(handleRequest)
}

func handleRequest(w http.ResponseWriter, r *http.Request) {
	w.Write([]byte("Hello from Go!\n"))
}

//go:generate go run github.com/bytecodealliance/wasm-tools-go/cmd/wit-bindgen-go generate --world hello --out gen ./wit
func main() {}
