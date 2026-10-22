package main

import (
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats.go"
	"github.com/nats-io/nkeys"
)

const (
	testOperatorUser = "operator"
	testTokenAuth    = "token"
)

// startNats starts an embedded NATS server configured by configure.
func startNats(t *testing.T, configure func(*server.Options)) string {
	t.Helper()

	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("reserve port: %v", err)
	}
	port := l.Addr().(*net.TCPAddr).Port
	_ = l.Close()

	opts := &server.Options{Host: "127.0.0.1", Port: port, NoLog: true, NoSigs: true}
	configure(opts)
	s, err := server.NewServer(opts)
	if err != nil {
		t.Fatalf("new nats server: %v", err)
	}
	s.Start()
	if !s.ReadyForConnections(10 * time.Second) {
		s.Shutdown()
		t.Fatal("nats server did not become ready")
	}
	t.Cleanup(func() {
		s.Shutdown()
		s.WaitForShutdown()
	})
	return fmt.Sprintf("nats://127.0.0.1:%d", port)
}

func writeSecret(t *testing.T, name, contents string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, []byte(contents), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

func connect(t *testing.T, url string, flags natsClientFlags) (*nats.Conn, error) {
	t.Helper()
	opts, err := flags.options()
	if err != nil {
		t.Fatalf("options: %v", err)
	}
	nc, err := nats.Connect(url, opts...)
	if err == nil {
		t.Cleanup(nc.Close)
	}
	return nc, err
}

func TestNatsClientAuthenticates(t *testing.T) {
	user, err := nkeys.CreateUser()
	if err != nil {
		t.Fatal(err)
	}
	seed, err := user.Seed()
	if err != nil {
		t.Fatal(err)
	}
	public, err := user.PublicKey()
	if err != nil {
		t.Fatal(err)
	}

	cases := []struct {
		name      string
		configure func(*server.Options)
		flags     natsClientFlags
	}{
		{
			name:      testTokenAuth,
			configure: func(o *server.Options) { o.Authorization = "s3cr3t" },
			flags:     natsClientFlags{tokenFile: writeSecret(t, "token", "s3cr3t\n")},
		},
		{
			name: "user and password",
			configure: func(o *server.Options) {
				o.Username = testOperatorUser
				o.Password = "s3cr3t"
			},
			flags: natsClientFlags{user: testOperatorUser, passwordFile: writeSecret(t, "password", "s3cr3t\n")},
		},
		{
			name:      "nkey",
			configure: func(o *server.Options) { o.Nkeys = []*server.NkeyUser{{Nkey: public}} },
			flags:     natsClientFlags{nkey: writeSecret(t, "operator.nk", string(seed))},
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			url := startNats(t, tc.configure)
			if _, err := connect(t, url, natsClientFlags{}); err == nil {
				t.Fatal("the server admitted an anonymous client")
			}
			tc.flags.name = "runtime-operator"
			nc, err := connect(t, url, tc.flags)
			if err != nil {
				t.Fatalf("connect: %v", err)
			}
			if nc.Opts.Name != "runtime-operator" {
				t.Fatalf("connection name %q", nc.Opts.Name)
			}
		})
	}
}

// With an inbox prefix only the operator may subscribe to, host replies reach
// the operator even though _INBOX.> is closed to it.
func TestNatsInboxPrefixCarriesHostReplies(t *testing.T) {
	url := startNats(t, func(o *server.Options) {
		o.Users = []*server.User{
			{
				Username: testOperatorUser,
				Password: testOperatorUser,
				Permissions: &server.Permissions{
					Publish:   &server.SubjectPermission{Allow: []string{"runtime.host.>"}},
					Subscribe: &server.SubjectPermission{Allow: []string{"runtime.operator.inbox.>"}},
				},
			},
			{
				Username: "host",
				Password: "host",
				Permissions: &server.Permissions{
					Publish:   &server.SubjectPermission{Deny: []string{">"}},
					Subscribe: &server.SubjectPermission{Allow: []string{"runtime.host.*.>"}},
					Response:  &server.ResponsePermission{MaxMsgs: 1},
				},
			},
		}
	})

	host, err := nats.Connect(url, nats.UserInfo("host", "host"))
	if err != nil {
		t.Fatalf("host connect: %v", err)
	}
	defer host.Close()
	if _, err := host.Subscribe("runtime.host.h1.heartbeat", func(m *nats.Msg) {
		_ = m.Respond([]byte("alive"))
	}); err != nil {
		t.Fatal(err)
	}
	if err := host.Flush(); err != nil {
		t.Fatal(err)
	}

	password := writeSecret(t, "password", testOperatorUser)
	withPrefix, err := connect(t, url, natsClientFlags{
		user: testOperatorUser, passwordFile: password, inboxPrefix: "runtime.operator.inbox",
	})
	if err != nil {
		t.Fatalf("operator connect: %v", err)
	}
	reply, err := withPrefix.Request("runtime.host.h1.heartbeat", nil, 2*time.Second)
	if err != nil {
		t.Fatalf("request through the operator inbox: %v", err)
	}
	if !strings.HasPrefix(reply.Subject, "runtime.operator.inbox.") || string(reply.Data) != "alive" {
		t.Fatalf("reply %q on %q", reply.Data, reply.Subject)
	}

	withoutPrefix, err := connect(t, url, natsClientFlags{user: testOperatorUser, passwordFile: password})
	if err != nil {
		t.Fatalf("operator connect: %v", err)
	}
	if _, err := withoutPrefix.Request("runtime.host.h1.heartbeat", nil, 500*time.Millisecond); err == nil {
		t.Fatal("a reply arrived on _INBOX, which the operator may not subscribe to")
	}
}

func TestNatsClientFlagsRefuseContradictions(t *testing.T) {
	const emptySecretError = "is empty"
	for _, tc := range []struct {
		name  string
		flags natsClientFlags
		want  string
	}{
		{"creds and nkey", natsClientFlags{creds: "a.creds", nkey: "a.nk"}, "set one"},
		{"password without user", natsClientFlags{passwordFile: "p"}, "requires -nats-user"},
		{"missing token file", natsClientFlags{tokenFile: "/nonexistent/token"}, "-nats-token-file"},
		{"empty token file", natsClientFlags{tokenFile: writeSecret(t, "empty", "\n")}, emptySecretError},
		{"zero-byte token file", natsClientFlags{tokenFile: writeSecret(t, "empty", "")}, emptySecretError},
		{"CRLF-only token file", natsClientFlags{tokenFile: writeSecret(t, "empty", "\r\n")}, emptySecretError},
		{"wildcard inbox", natsClientFlags{inboxPrefix: "runtime.*"}, "wildcards"},
		{"empty inbox token", natsClientFlags{inboxPrefix: "runtime..inbox"}, "empty subject token"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, err := tc.flags.options()
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("got %v, want an error containing %q", err, tc.want)
			}
		})
	}
}

func TestNatsSecretFilesReload(t *testing.T) {
	token := writeSecret(t, "token", "first-token\n")
	password := writeSecret(t, "password", "first-password\n")
	flags := natsClientFlags{tokenFile: token, user: testOperatorUser, passwordFile: password}
	options, err := flags.options()
	if err != nil {
		t.Fatal(err)
	}
	opts := nats.GetDefaultOptions()
	for _, option := range options {
		if err := option(&opts); err != nil {
			t.Fatal(err)
		}
	}
	if got := opts.TokenHandler(); got != "first-token" {
		t.Fatal("initial token was not loaded")
	}
	if user, pass := opts.UserInfo(); user != testOperatorUser || pass != "first-password" {
		t.Fatal("initial user credentials were not loaded")
	}
	if err := os.WriteFile(token, []byte("second-token\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(password, []byte("second-password\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := opts.TokenHandler(); got != "second-token" {
		t.Fatal("token rotation was not loaded")
	}
	if user, pass := opts.UserInfo(); user != testOperatorUser || pass != "second-password" {
		t.Fatal("password rotation was not loaded")
	}
}

func TestNatsSecretFilesPreserveWhitespace(t *testing.T) {
	const boundarySecret = " \tsecret\t "
	cases := []struct {
		name       string
		secret     string
		lineEnding string
	}{
		{"boundary whitespace", boundarySecret, ""},
		{"whitespace only", " \t ", ""},
		{"LF ending", boundarySecret, "\n"},
		{"CRLF ending", boundarySecret, "\r\n"},
		{"bare carriage return", "secret\r", ""},
		{"one line ending", "secret\n", "\n"},
	}
	for _, method := range []string{testTokenAuth, "password"} {
		t.Run(method, func(t *testing.T) {
			for _, tc := range cases {
				t.Run(tc.name, func(t *testing.T) {
					url := startNats(t, func(o *server.Options) {
						if method == testTokenAuth {
							o.Authorization = tc.secret
						} else {
							o.Username = testOperatorUser
							o.Password = tc.secret
						}
					})
					path := writeSecret(t, method, tc.secret+tc.lineEnding)
					flags := natsClientFlags{tokenFile: path}
					if method == "password" {
						flags = natsClientFlags{user: testOperatorUser, passwordFile: path}
					}
					if _, err := connect(t, url, flags); err != nil {
						t.Fatalf("connect with whitespace-preserving secret file: %v", err)
					}
				})
			}
		})
	}
}
