package main

import (
	"context"
	"net"
	"net/http"
	"net/http/httputil"
	"time"

	ctrl "sigs.k8s.io/controller-runtime"
)

// How long the gateway lets the requests already in flight finish once it is
// told to stop. The pod's terminationGracePeriodSeconds has to clear it, or
// SIGKILL lands mid-response; .github/scripts/check-termination-grace.mjs
// reads this and checks that it does.
const gracefulShutdownTimeout = 15 * time.Second

type HTTPGateway struct {
	BindAddr string
	Proxy    *httputil.ReverseProxy
	Resolver HostResolver
}

func (h *HTTPGateway) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	h.Proxy.ModifyResponse = h.modifyResponse
	h.Proxy.Rewrite = h.rewrite
	h.Proxy.ErrorHandler = h.proxyErrorHandler
	return manager.Add(h)
}

func (h *HTTPGateway) Start(ctx context.Context) error {
	log := ctrl.LoggerFrom(ctx).WithName("http-gateway")

	// Requests take the manager's context values — its logger — but not its
	// cancellation. Cancelling it is what starts the shutdown, and handing that
	// same context to the handlers cancels every request the drain below exists
	// to let finish, answering each one 502 instead.
	serving := context.WithoutCancel(ctx)

	publicFacing := &http.Server{
		BaseContext: func(_ net.Listener) context.Context {
			return serving
		},
		Addr:              h.BindAddr,
		Handler:           h.Proxy,
		IdleTimeout:       0,
		ReadTimeout:       0,
		WriteTimeout:      0,
		ReadHeaderTimeout: 10 * time.Second,
	}

	drained := make(chan struct{})
	go func() {
		defer close(drained)
		<-ctx.Done()
		shutdownCtx, cancel := context.WithTimeout(context.Background(), gracefulShutdownTimeout)
		defer cancel()

		if err := publicFacing.Shutdown(shutdownCtx); err != nil {
			log.Error(err, "HTTP server shutdown error")
		}

	}()

	log.Info("Starting HTTP Gateway", "bindAddr", h.BindAddr)
	if err := publicFacing.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		return err
	}

	// Shutdown closes the listener first, so ListenAndServe returns while the
	// requests already in flight are still being served. Returning here ends
	// this runnable, and with it the manager and the process, hanging up on
	// every one of them.
	<-drained

	return nil

}

func (h *HTTPGateway) rewrite(req *httputil.ProxyRequest) {
	// X-Forwarded-For handling
	clientIP := req.In.RemoteAddr
	if xff := req.In.Header.Get("X-Forwarded-For"); xff != "" {
		clientIP = xff
		req.Out.Header.Set("X-Forwarded-For", xff)
	}
	req.Out.Header.Set("X-Real-IP", clientIP)
	req.SetXForwarded()

	// Preserve Connection header from the original request
	if connection := req.In.Header.Get("Connection"); connection != "" {
		req.Out.Header.Set("Connection", connection)
	}

	// Workload Lookup
	lookupRes := h.Resolver.Resolve(req.In.Context(), req.In)

	originalURL := req.In.URL
	originalURL.Host = lookupRes.Hostname
	originalURL.Scheme = lookupRes.Scheme
	// make sure we keep public url information intact. important for query string hashing.
	req.Out.URL = originalURL
	if lookupRes.WorkloadID != "" {
		req.Out.Header.Set("X-Workload-Id", lookupRes.WorkloadID)
	}

	req.Out.Host = req.In.Host
}

func (h *HTTPGateway) modifyResponse(resp *http.Response) error {
	return nil
}

func (h *HTTPGateway) proxyErrorHandler(w http.ResponseWriter, req *http.Request, err error) {
	log := ctrl.LoggerFrom(req.Context()).WithName("http-gateway")
	log.Error(err, "proxy error")

	w.Header().Set("Connection", "close")
	http.Error(w, "Gateway Error", http.StatusBadGateway)
}
