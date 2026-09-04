package main

import (
	"context"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"net/url"
	"sync"
	"testing"
	"time"
)

// freePort returns an address nothing is listening on, for a server that takes
// its address as a string rather than a listener.
func freePort(t *testing.T) string {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("reserve a port: %v", err)
	}
	addr := l.Addr().String()
	if err := l.Close(); err != nil {
		t.Fatalf("release the reserved port: %v", err)
	}
	return addr
}

type proxiedResponse struct {
	status int
	err    error
}

// A terminating gateway has to finish what it is already serving. The gateway
// is a controller-runtime runnable, so the manager waits for Start to return
// and the process exits behind it — and the shutdown it runs closes the
// listener before it drains, which is what makes both halves of this easy to
// get wrong: returning as soon as the listener closes ends the process with
// requests in flight, and handing those requests a context that the shutdown
// itself cancels aborts them where they stand.
func TestStartWaitsForInFlightRequests(t *testing.T) {
	var releaseOnce sync.Once
	released := make(chan struct{})
	release := func() { releaseOnce.Do(func() { close(released) }) }

	received := make(chan struct{})
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		close(received)
		select {
		case <-released:
			w.WriteHeader(http.StatusOK)
		case <-r.Context().Done():
		}
	}))
	// Registered before the release below so cleanup runs it second: closing
	// the backend waits for its handlers, which are waiting on the release.
	t.Cleanup(backend.Close)
	t.Cleanup(release)

	backendURL, err := url.Parse(backend.URL)
	if err != nil {
		t.Fatalf("parse backend URL: %v", err)
	}

	addr := freePort(t)
	gateway := &HTTPGateway{
		BindAddr: addr,
		Proxy: &httputil.ReverseProxy{
			Rewrite: func(r *httputil.ProxyRequest) { r.SetURL(backendURL) },
		},
	}

	ctx, stop := context.WithCancel(context.Background())
	t.Cleanup(stop)
	started := make(chan error, 1)
	go func() { started <- gateway.Start(ctx) }()

	waitForListener(t, addr)

	responded := make(chan proxiedResponse, 1)
	go func() {
		resp, err := http.Get("http://" + addr + "/")
		if err != nil {
			responded <- proxiedResponse{err: err}
			return
		}
		defer func() { _ = resp.Body.Close() }()
		// Read it out so the connection goes idle and the drain can finish.
		if _, err := io.Copy(io.Discard, resp.Body); err != nil {
			responded <- proxiedResponse{err: err}
			return
		}
		responded <- proxiedResponse{status: resp.StatusCode}
	}()

	select {
	case <-received:
	case <-time.After(5 * time.Second):
		t.Fatal("the backend never received the proxied request")
	}

	// The pod is terminating with that request still being served.
	stop()

	select {
	case err := <-started:
		t.Fatalf("Start returned (%v) with a request still in flight; the process exits behind it", err)
	case got := <-responded:
		t.Fatalf("the in-flight request was cut short by the shutdown: %+v", got)
	case <-time.After(250 * time.Millisecond):
	}

	release()

	got := <-responded
	if got.err != nil {
		t.Fatalf("the in-flight request did not complete: %v", got.err)
	}
	if got.status != http.StatusOK {
		t.Errorf("in-flight request finished with %d, want %d", got.status, http.StatusOK)
	}

	select {
	case err := <-started:
		if err != nil {
			t.Errorf("Start returned %v, want nil once drained", err)
		}
	case <-time.After(gracefulShutdownTimeout):
		t.Fatal("Start never returned after the drain finished")
	}
}

func waitForListener(t *testing.T, addr string) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("tcp", addr, 100*time.Millisecond)
		if err == nil {
			if err := conn.Close(); err != nil {
				t.Fatalf("close the probe connection: %v", err)
			}
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("the gateway never listened on %s", addr)
}
