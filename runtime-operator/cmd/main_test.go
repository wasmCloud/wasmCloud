/*
Copyright 2024.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

	http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package main

import (
	"fmt"
	"net"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/server"

	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// startNatsOnFreePort starts an embedded NATS server on an ephemeral port to
// avoid contention with the fixed-port servers used by the pkg/wasmbus tests,
// which may run in parallel.
func startNatsOnFreePort(t *testing.T) (url string, stop func()) {
	t.Helper()

	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("reserve port: %v", err)
	}
	port := l.Addr().(*net.TCPAddr).Port
	_ = l.Close()

	s, err := server.NewServer(&server.Options{
		Host:   "127.0.0.1",
		Port:   port,
		NoLog:  true,
		NoSigs: true,
	})
	if err != nil {
		t.Fatalf("new nats server: %v", err)
	}
	s.Start()
	if !s.ReadyForConnections(10 * time.Second) {
		s.Shutdown()
		t.Fatal("nats server did not become ready")
	}

	return fmt.Sprintf("nats://127.0.0.1:%d", port), func() {
		s.Shutdown()
		s.WaitForShutdown()
	}
}

// TestNatsLivenessCheck verifies the operator's liveness reflects NATS
// connectivity: healthy while connected, and unhealthy once the connection is
// permanently closed so the kubelet restarts the pod.
func TestNatsLivenessCheck(t *testing.T) {
	url, stop := startNatsOnFreePort(t)
	defer stop()

	nc, err := wasmbus.NatsConnect(url)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	defer nc.Close()

	check := natsLivenessCheck(nc)

	if err := check(nil); err != nil {
		t.Fatalf("expected healthy while connected, got: %v", err)
	}

	nc.Close()
	deadline := time.Now().Add(5 * time.Second)
	for !nc.IsClosed() && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if err := check(nil); err == nil {
		t.Fatal("expected liveness to fail on a permanently closed nats connection")
	}
}
