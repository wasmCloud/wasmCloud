package wasmbus

import (
	"context"
	"errors"
	"fmt"
	"net"
	"strings"
	"syscall"
	"testing"
	"testing/synctest"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats.go"
)

type refusedNatsDialer struct{ dialed chan<- struct{} }

func (d refusedNatsDialer) Dial(string, string) (net.Conn, error) {
	select {
	case d.dialed <- struct{}{}:
	default:
	}
	return nil, syscall.ECONNREFUSED
}

func TestNatsConnectContextCanceledOnEntry(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	nc, err := NatsConnectContext(ctx, NatsDefaultURL, nats.SetCustomDialer(refusedNatsDialer{}))
	if nc != nil {
		nc.Close()
		t.Fatal("cancelled startup returned a connection")
	}
	if !errors.Is(err, ErrTransport) || !strings.Contains(err.Error(), context.Canceled.Error()) {
		t.Fatalf("cancelled startup = %v, want a transport error with cancellation details", err)
	}
}

func TestNatsConnectContextCancelsStartupWait(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		defer cancel()
		type result struct {
			conn *nats.Conn
			err  error
		}
		dialed := make(chan struct{}, 1)
		done := make(chan result, 1)
		go func() {
			nc, err := NatsConnectContext(ctx, NatsDefaultURL, nats.SetCustomDialer(refusedNatsDialer{dialed: dialed}))
			done <- result{nc, err}
		}()
		select {
		case <-dialed:
		case got := <-done:
			if got.conn != nil {
				got.conn.Close()
			}
			t.Fatalf("returned before a failed dial: %v", got.err)
		case <-time.After(time.Second):
			t.Fatal("no dial attempt")
		}
		synctest.Wait()
		cancel()
		select {
		case got := <-done:
			if got.conn != nil {
				got.conn.Close()
				t.Fatal("canceled startup returned a connection")
			}
			if !errors.Is(got.err, ErrTransport) || !strings.Contains(got.err.Error(), context.Canceled.Error()) {
				t.Fatalf("startup wait = %v, want cancellation", got.err)
			}
		case <-time.After(100 * time.Millisecond):
			t.Fatal("cancellation did not interrupt the startup backoff")
		}
	})
}

func TestNatsRetryClassification(t *testing.T) {
	for _, tc := range []struct {
		name  string
		err   error
		retry bool
	}{
		{"no servers", nats.ErrNoServers, true},
		{"timeout", nats.ErrTimeout, true},
		{"connection refused", syscall.ECONNREFUSED, true},
		{"host unreachable", syscall.EHOSTUNREACH, true},
		{"network unreachable", syscall.ENETUNREACH, true},
		{"DNS not found", &net.DNSError{IsNotFound: true}, true},
		{"temporary DNS failure", &net.DNSError{IsTemporary: true}, true},
		{"permanent DNS failure", &net.DNSError{Err: "invalid name"}, false},
		{"authorization", nats.ErrAuthorization, false},
		{"other failure", errors.New("invalid configuration"), false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			for _, err := range []error{tc.err, fmt.Errorf("connect: %w", tc.err)} {
				if got := worthRetrying(err); got != tc.retry {
					t.Fatalf("worthRetrying(%v) = %t, want %t", err, got, tc.retry)
				}
			}
		})
	}
}

func TestNatsEmbeddedServerSupportsJetStream(t *testing.T) {
	opts := NatsDefaultServerOptions()
	opts.Host = "127.0.0.1"
	opts.Port = server.RANDOM_PORT
	opts.StoreDir = t.TempDir()
	s, err := NatsEmbeddedServer(opts, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Shutdown(); s.WaitForShutdown() })
	nc, err := nats.Connect(s.ClientURL())
	if err != nil {
		t.Fatal(err)
	}
	defer nc.Close()
	js, err := nc.JetStream(nats.Domain(opts.JetStreamDomain))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := js.AccountInfo(); err != nil {
		t.Fatalf("embedded JetStream is unavailable: %v", err)
	}
}

func TestNatsEmbeddedServerRejectsInvalidOptions(t *testing.T) {
	s, err := NatsEmbeddedServer(&server.Options{ServerName: "invalid name", NoSigs: true}, time.Second)
	if s != nil {
		s.Shutdown()
		t.Fatal("invalid options returned a server")
	}
	if err == nil {
		t.Fatal("invalid options did not return an error")
	}
}

func TestNatsEmbeddedServerStartupTimeout(t *testing.T) {
	s, err := NatsEmbeddedServer(&server.Options{DontListen: true, NoSigs: true}, 0)
	if s != nil {
		s.Shutdown()
		t.Fatal("expired startup returned a server")
	}
	if err == nil {
		t.Fatal("expired startup did not return an error")
	}
}
