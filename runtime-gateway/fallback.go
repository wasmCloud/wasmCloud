package main

import (
	"context"
	"errors"
	"net"
	"net/http"

	ctrl "sigs.k8s.io/controller-runtime"
)

// Fallback defines an interface for obtaining a fallback endpoint
type Fallback interface {
	// InvalidHostname returns the scheme and endpoint to use as a fallback when the hostname is not recognized
	InvalidHostname(hostname string) (scheme string, endpoint string)
	// NoWorkloads returns the scheme and endpoint to use when there are no workloads registered for a hostname
	NoWorkloads(hostname string) (scheme string, endpoint string)
}

var _ Fallback = (*ExternalFallback)(nil)

// ExternalFallback represents an externally provided fallback endpoint
type ExternalFallback struct {
	Scheme   string
	Endpoint string
}

func (ef *ExternalFallback) InvalidHostname(hostname string) (string, string) {
	return ef.Scheme, ef.Endpoint
}

func (ef *ExternalFallback) NoWorkloads(hostname string) (string, string) {
	return ef.Scheme, ef.Endpoint
}

var _ Fallback = (*FallbackServer)(nil)

// FallbackServer provides a simple HTTP server that responds with a 503 Service Unavailable
// when no workloads are registered to handle incoming requests.
type FallbackServer struct {
	listener net.Listener
}

func newFallbackServer(bindAddr string) (*FallbackServer, error) {
	listener, err := net.Listen("tcp", bindAddr)
	if err != nil {
		return nil, err
	}
	return &FallbackServer{listener: listener}, nil
}

func (fs *FallbackServer) Close() error {
	return fs.listener.Close()
}

func (fs *FallbackServer) InvalidHostname(hostname string) (string, string) {
	return "http", fs.listener.Addr().String()
}

func (fs *FallbackServer) NoWorkloads(hostname string) (string, string) {
	return fs.InvalidHostname(hostname)
}

func (fs *FallbackServer) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	return manager.Add(fs)
}

func (fs *FallbackServer) Start(ctx context.Context) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	internalMux := http.NewServeMux()

	internalMux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		log := ctrl.LoggerFrom(r.Context()).WithName("fallback")
		log.Info("no backend available for request", "host", r.Host, "path", r.URL.Path)

		w.Header().Set("Connection", "close")
		http.Error(w, "No backend available", http.StatusServiceUnavailable)
	})

	log := ctrl.LoggerFrom(ctx).WithName("fallback")
	log.Info("starting", "endpoint", fs.listener.Addr().String())

	internalServer := &http.Server{
		Handler: internalMux,
		BaseContext: func(_ net.Listener) context.Context {
			return ctx
		},
	}
	stopped := make(chan struct{})
	go func() {
		defer close(stopped)
		<-ctx.Done()
		_ = internalServer.Close()
	}()

	err := internalServer.Serve(fs.listener)
	cancel()
	<-stopped
	if errors.Is(err, http.ErrServerClosed) || errors.Is(err, net.ErrClosed) {
		return nil
	}
	return err
}
