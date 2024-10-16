package main

import (
	"fmt"

	gen "github.com/wasmcloud/wasmcloud/examples/golang/providers/custom-template/component/gen"
)

type CustomTemplateComponent struct{}

func init() {
	customTemplate := CustomTemplateComponent{}
	gen.SetExportsWasmcloudExample0_1_0_ProcessData(customTemplate)
}

func (c CustomTemplateComponent) Process(data gen.ExportsWasmcloudExample0_1_0_ProcessDataData) string {
	gen.WasiLogging0_1_0_draft_LoggingLog(gen.WasiLogging0_1_0_draft_LoggingLevelInfo(), "", fmt.Sprintf("Processing data: %v", data))
	os := gen.WasmcloudExample0_1_0_SystemInfoRequestInfo(gen.WasmcloudExample0_1_0_SystemInfoKindOs())
	arch := gen.WasmcloudExample0_1_0_SystemInfoRequestInfo(gen.WasmcloudExample0_1_0_SystemInfoKindArch())
	return fmt.Sprintf("Provider is running on %s-%s", os, arch)
}

//go:generate wit-bindgen tiny-go wit --out-dir=gen --gofmt
func main() {}
