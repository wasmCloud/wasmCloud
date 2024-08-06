//go:generate wit-bindgen-wrpc go --out-dir bindings --package go.wasmcloud.dev/provider/examples/keyvalue-inmemory/bindings wit

package main

import (
	"log"
	"os"
	"os/signal"
	"syscall"

	"go.opentelemetry.io/otel"
	"go.wasmcloud.dev/provider"
	server "go.wasmcloud.dev/provider/examples/keyvalue-inmemory/bindings"
)

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}

func run() error {
	p := &Provider{
		sourceLinks: make(map[string]provider.InterfaceLinkDefinition),
		targetLinks: make(map[string]provider.InterfaceLinkDefinition),
		tracer:      otel.Tracer("keyvalue-inmemory"),
	}

	wasmcloudprovider, err := provider.New(
		provider.SourceLinkPut(p.handleNewSourceLink),
		provider.TargetLinkPut(p.handleNewTargetLink),
		provider.SourceLinkDel(p.handleDelSourceLink),
		provider.TargetLinkDel(p.handleDelTargetLink),
		provider.HealthCheck(p.handleHealthCheck),
		provider.Shutdown(p.handleShutdown),
	)
	if err != nil {
		return err
	}

	providerCh := make(chan error, 1)
	signalCh := make(chan os.Signal, 1)

	// Handle RPC operations
	stopFunc, err := server.Serve(wasmcloudprovider.RPCClient, p)
	if err != nil {
		wasmcloudprovider.Shutdown()
		return err
	}

	// Handle control interface operations
	go func() {
		err := wasmcloudprovider.Start()
		providerCh <- err
	}()

	// Shutdown on SIGINT
	signal.Notify(signalCh, syscall.SIGINT)

	select {
	case err = <-providerCh:
		stopFunc()
		return err
	case <-signalCh:
		wasmcloudprovider.Shutdown()
		stopFunc()
	}

	return nil
}

func (p *Provider) handleNewSourceLink(link provider.InterfaceLinkDefinition) error {
	log.Println("Handling new source link", link)
	p.sourceLinks[link.Target] = link
	return nil
}

func (p *Provider) handleNewTargetLink(link provider.InterfaceLinkDefinition) error {
	log.Println("Handling new target link", link)
	p.targetLinks[link.SourceID] = link
	return nil
}

func (p *Provider) handleDelSourceLink(link provider.InterfaceLinkDefinition) error {
	log.Println("Handling del source link", link)
	delete(p.sourceLinks, link.Target)
	return nil
}

func (p *Provider) handleDelTargetLink(link provider.InterfaceLinkDefinition) error {
	log.Println("Handling del target link", link)
	delete(p.targetLinks, link.SourceID)
	return nil
}

func (p *Provider) handleHealthCheck() string {
	log.Println("Handling health check")
	return "provider healthy"
}

func (p *Provider) handleShutdown() error {
	log.Println("Handling shutdown")
	return nil
}
