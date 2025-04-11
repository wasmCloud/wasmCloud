//go:generate go tool wit-bindgen-go generate --world component --out gen ./wit
package main

import (
	"fmt"

	"github.com/wasmcloud/wasmcloud/examples/golang/providers/custom-template/component/gen/wasi/logging/logging"
	process "github.com/wasmcloud/wasmcloud/examples/golang/providers/custom-template/component/gen/wasmcloud/example/process-data"
	system "github.com/wasmcloud/wasmcloud/examples/golang/providers/custom-template/component/gen/wasmcloud/example/system-info"
)

func init() {
	process.Exports.Process = Process
}

func Process(data process.Data) string {
	logging.Log(logging.LevelInfo, "", fmt.Sprintf("Processing data: %v", data))
	os := system.RequestInfo(system.KindOS)
	arch := system.RequestInfo(system.KindOS)
	return fmt.Sprintf("Provider is running on %s-%s", os, arch)
}

func main() {}
