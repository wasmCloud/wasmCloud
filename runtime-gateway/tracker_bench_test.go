package main

import (
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/http/httputil"
	"strconv"
	"testing"

	"k8s.io/apimachinery/pkg/types"
)

// benchID returns a UUID-shaped ID, as hosts and workloads carry.
func benchID(i int) string {
	return fmt.Sprintf("%08x-0000-4000-8000-%012x", i, i)
}

// One hostname served by many replicas, each on its own host, with all, one or
// none of those hosts Ready.
func BenchmarkResolve(b *testing.B) {
	for _, replicas := range []int{10, 100, 1000} {
		for _, ready := range []string{"all", "one", "none"} {
			b.Run(fmt.Sprintf("replicas=%d/ready=%s", replicas, ready), func(b *testing.B) {
				ctx := b.Context()
				ht := newHostTracker(testFallback{})
				for i := range replicas {
					hostID, workloadID := benchID(2*i), benchID(2*i+1)
					key := types.NamespacedName{Namespace: "ns", Name: workloadID}
					if err := ht.RegisterHost(ctx, key, hostID, "10.0.0.1", 8080); err != nil {
						b.Fatal(err)
					}
					if err := ht.RegisterWorkload(ctx, key, hostID, workloadID, "app.example"); err != nil {
						b.Fatal(err)
					}
					if ready == "none" || (ready == "one" && i > 0) {
						if err := ht.SuspendHost(ctx, key, hostID); err != nil {
							b.Fatal(err)
						}
					}
				}
				req := &http.Request{Host: "app.example"}
				b.ReportAllocs()
				for b.Loop() {
					ht.Resolve(ctx, req)
				}
			})
		}
	}
}

// A whole request through the gateway's proxy to a backend on loopback, for
// scale against BenchmarkResolve.
func BenchmarkProxiedRequest(b *testing.B) {
	ctx := b.Context()
	backend := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNoContent)
	}))
	b.Cleanup(backend.Close)
	host, port, err := net.SplitHostPort(backend.Listener.Addr().String())
	if err != nil {
		b.Fatal(err)
	}
	portNum, err := strconv.Atoi(port)
	if err != nil {
		b.Fatal(err)
	}

	ht := newHostTracker(testFallback{})
	key := types.NamespacedName{Namespace: "ns", Name: "app"}
	if err := ht.RegisterHost(ctx, key, benchID(0), host, portNum); err != nil {
		b.Fatal(err)
	}
	if err := ht.RegisterWorkload(ctx, key, benchID(0), benchID(1), "app.example"); err != nil {
		b.Fatal(err)
	}
	gateway := &HTTPGateway{Proxy: &httputil.ReverseProxy{}, Resolver: ht}
	gateway.Proxy.Rewrite = gateway.rewrite
	front := httptest.NewServer(gateway.Proxy)
	b.Cleanup(front.Close)

	client := front.Client()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, front.URL, nil)
	if err != nil {
		b.Fatal(err)
	}
	req.Host = "app.example"
	b.ReportAllocs()
	for b.Loop() {
		resp, err := client.Do(req)
		if err != nil {
			b.Fatal(err)
		}
		_, _ = io.Copy(io.Discard, resp.Body)
		_ = resp.Body.Close()
		if resp.StatusCode != http.StatusNoContent {
			b.Fatalf("status = %d", resp.StatusCode)
		}
	}
}

// The write side, for one hostname served by many replicas: a host leaving and
// rejoining traffic, a workload registering unchanged, and one moving hosts.
func BenchmarkRegistrationChange(b *testing.B) {
	for _, replicas := range []int{10, 100, 1000} {
		b.Run(fmt.Sprintf("replicas=%d", replicas), func(b *testing.B) {
			ctx := b.Context()
			ht := newHostTracker(testFallback{})
			for i := range replicas {
				hostID, workloadID := benchID(2*i), benchID(2*i+1)
				key := types.NamespacedName{Namespace: "ns", Name: workloadID}
				if err := ht.RegisterHost(ctx, key, hostID, "10.0.0.1", 8080); err != nil {
					b.Fatal(err)
				}
				if err := ht.RegisterWorkload(ctx, key, hostID, workloadID, "app.example"); err != nil {
					b.Fatal(err)
				}
			}
			hostID, workloadID := benchID(0), benchID(1)
			key := types.NamespacedName{Namespace: "ns", Name: workloadID}
			b.Run("host=suspend+register", func(b *testing.B) {
				for b.Loop() {
					if err := ht.SuspendHost(ctx, key, hostID); err != nil {
						b.Fatal(err)
					}
					if err := ht.RegisterHost(ctx, key, hostID, "10.0.0.1", 8080); err != nil {
						b.Fatal(err)
					}
				}
			})
			b.Run("workload=unchanged", func(b *testing.B) {
				for b.Loop() {
					if err := ht.RegisterWorkload(ctx, key, hostID, workloadID, "app.example"); err != nil {
						b.Fatal(err)
					}
				}
			})
			b.Run("workload=move", func(b *testing.B) {
				for b.Loop() {
					if err := ht.RegisterWorkload(ctx, key, benchID(2), workloadID, "app.example"); err != nil {
						b.Fatal(err)
					}
					if err := ht.RegisterWorkload(ctx, key, hostID, workloadID, "app.example"); err != nil {
						b.Fatal(err)
					}
				}
			})
		})
	}
}
