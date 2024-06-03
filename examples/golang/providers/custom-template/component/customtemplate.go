package main

import (
	"fmt"

	gen "github.com/wasmcloud/wasmcloud/examples/golang/providers/custom-template/component/gen"
)

type CustomTemplateComponent struct{}

func init() {
	customTemplate := CustomTemplateComponent{}
	gen.SetExportsWasmcloudExampleProcessData(customTemplate)
}

func (c CustomTemplateComponent) Process(data gen.ExportsWasmcloudExampleProcessDataData) string {
	gen.WasiLoggingLoggingLog(gen.WasiLoggingLoggingLevelInfo(), "", fmt.Sprintf("Processing data: %v", data))
	os := gen.WasmcloudExampleSystemInfoRequestInfo(gen.WasmcloudExampleSystemInfoKindOs())
	arch := gen.WasmcloudExampleSystemInfoRequestInfo(gen.WasmcloudExampleSystemInfoKindArch())
	return fmt.Sprintf("Provider is running on %s-%s", os, arch)
}

//go:generate wit-bindgen tiny-go wit --out-dir=gen --gofmt
func main() {}
