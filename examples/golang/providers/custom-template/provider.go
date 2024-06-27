package main

import (
	"context"
	"errors"
	"runtime"

	// Go provider SDK
	sdk "github.com/wasmCloud/provider-sdk-go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"

	// Generated bindings from the wit world
	system_info "github.com/wasmCloud/wasmCloud/examples/go/providers/custom-template/bindings/exports/wasmcloud/example/system_info"
	"github.com/wasmCloud/wasmCloud/examples/go/providers/custom-template/bindings/wasmcloud/example/process_data"
)

// / Your Handler struct is where you can store any state or configuration that your provider needs to keep track of.
type Handler struct {
	// The provider instance
	provider *sdk.WasmcloudProvider
	// All components linked to this provider and their config.
	linkedFrom map[string]map[string]string
	// All components this provider is linked to and their config
	linkedTo map[string]map[string]string
}

// Request information about the system the provider is running on
func (h *Handler) RequestInfo(ctx context.Context, kind system_info.Kind) (string, error) {
	// Only allow requests from a lattice source
	header, ok := wrpcnats.HeaderFromContext(ctx)
	if !ok {
		h.provider.Logger.Warn("Received request from unknown origin")
		return "", nil
	}
	// Only allow requests from a linked component
	sourceId := header.Get("source-id")
	if h.linkedFrom[sourceId] == nil {
		h.provider.Logger.Warn("Received request from unlinked source", "sourceId", sourceId)
		return "", nil
	}

	h.provider.Logger.Debug("Received request for system information", "sourceId", sourceId)

	switch kind {
	case system_info.Kind_Os:
		return runtime.GOOS, nil
	case system_info.Kind_Arch:
		return runtime.GOARCH, nil
	default:
		return "", errors.New("invalid system info request")
	}
}

// Example export to call from the provider for testing
func (h *Handler) Call(ctx context.Context) (string, error) {
	var lastResponse string
	for target := range h.linkedTo {
		data := process_data.Data{
			Count: 3,
			Name:  "sup",
		}
		// Get the outgoing RPC client for the target
		client := h.provider.OutgoingRpcClient(target)
		// Send the data to the target for processing
		res, close, err := process_data.Process(ctx, client, &data)
		defer close()
		if err != nil {
			return "", err
		}
		lastResponse = res
	}

	if lastResponse == "" {
		lastResponse = "Provider received call but was not linked to any components"
	}

	return lastResponse, nil
}
