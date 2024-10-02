//go:build tools

package main

import (
	_ "github.com/bytecodealliance/wasm-tools-go/cmd/wit-bindgen-go"
	_ "go.wasmcloud.dev/wadge/cmd/wadge-bindgen-go"
)
