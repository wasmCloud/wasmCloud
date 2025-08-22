//go:generate go tool wit-bindgen-go generate --world component --out gen ./wit
package main

import (
	"fmt"

	"go.wasmcloud.dev/component/log/wasilog"

	process "github.com/wasmcloud/wasmcloud/examples/golang/providers/custom-template/component/gen/wasmcloud/example/process-data"
	system "github.com/wasmcloud/wasmcloud/examples/golang/providers/custom-template/component/gen/wasmcloud/example/system-info"
)

func init() {
	process.Exports.Process = Process
}

func Process(data process.Data) string {
	logger := wasilog.ContextLogger("handle")
	logger.Info(fmt.Sprintf("Processing data: %v", data))
	os := system.RequestInfo(system.KindOS)
	arch := system.RequestInfo(system.KindARCH)
	return fmt.Sprintf("Provider is running on %s-%s", os, arch)
}

func main() {}
