package natsclient

import (
	"bytes"
	"flag"
	"strings"
	"testing"
	"time"

	"github.com/go-logr/logr/funcr"
	"github.com/nats-io/jwt/v2"
	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nkeys"
)

// operatorServer starts a server in operator (JWT) mode trusting one account,
// and returns a creds file for a user of that account.
func operatorServer(t *testing.T) (url, creds string) {
	t.Helper()
	operator, err := nkeys.CreateOperator()
	if err != nil {
		t.Fatal(err)
	}
	operatorPublic, _ := operator.PublicKey()
	operatorJWT, err := jwt.NewOperatorClaims(operatorPublic).Encode(operator)
	if err != nil {
		t.Fatal(err)
	}
	operatorClaims, err := jwt.DecodeOperatorClaims(operatorJWT)
	if err != nil {
		t.Fatal(err)
	}

	account, err := nkeys.CreateAccount()
	if err != nil {
		t.Fatal(err)
	}
	accountPublic, _ := account.PublicKey()
	accountJWT, err := jwt.NewAccountClaims(accountPublic).Encode(operator)
	if err != nil {
		t.Fatal(err)
	}

	user, err := nkeys.CreateUser()
	if err != nil {
		t.Fatal(err)
	}
	userPublic, _ := user.PublicKey()
	userSeed, _ := user.Seed()
	userJWT, err := jwt.NewUserClaims(userPublic).Encode(account)
	if err != nil {
		t.Fatal(err)
	}
	config, err := jwt.FormatUserConfig(userJWT, userSeed)
	if err != nil {
		t.Fatal(err)
	}

	resolver := &server.MemAccResolver{}
	if err := resolver.Store(accountPublic, accountJWT); err != nil {
		t.Fatal(err)
	}
	url = startNats(t, func(o *server.Options) {
		o.TrustedOperators = []*jwt.OperatorClaims{operatorClaims}
		o.AccountResolver = resolver
	})
	return url, writeSecret(t, "operator.creds", string(config))
}

// A creds file (JWT plus seed) authenticates against an operator-mode server,
// and a refused credential does not abort reconnects: kubelet may yet refresh
// the mounted Secret.
func TestNatsCredsFileAuthenticates(t *testing.T) {
	url, creds := operatorServer(t)
	if _, err := connect(t, url, Flags{}); err == nil {
		t.Fatal("the server admitted an anonymous client")
	}
	nc, err := connect(t, url, Flags{Creds: creds})
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	if !nc.Opts.IgnoreAuthErrorAbort {
		t.Error("a creds-file client aborts reconnects on the first auth error")
	}
}

func TestNatsClientFlagsRejectUnreadableSecrets(t *testing.T) {
	for _, tc := range []struct {
		name  string
		flags Flags
		want  string
	}{
		{"nkey file missing", Flags{Nkey: "/nonexistent/operator.nk"}, "-nats-nkey"},
		{"nkey file is not a seed", Flags{Nkey: writeSecret(t, "bad.nk", "not-a-seed\n")}, "-nats-nkey"},
		{
			"password file missing",
			Flags{User: testOperatorUser, PasswordFile: "/nonexistent/password"}, "-nats-password-file",
		},
		{"password file empty", Flags{User: testOperatorUser, PasswordFile: writeSecret(t, "password", "\n")}, "is empty"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			_, err := tc.flags.Options()
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("got %v, want an error containing %q", err, tc.want)
			}
		})
	}
}

// A grace set before Register is the flag's default rather than being
// replaced by DefaultAuthFailureGrace.
func TestNatsClientFlagsKeepPresetGrace(t *testing.T) {
	fs := flag.NewFlagSet("embedder", flag.ContinueOnError)
	flags := Flags{AuthFailureGrace: 3 * time.Minute}
	flags.Register(fs, "")
	if flags.AuthFailureGrace != 3*time.Minute {
		t.Fatalf("auth failure grace %v, want the preset", flags.AuthFailureGrace)
	}
	if got := fs.Lookup("nats-auth-failure-grace").DefValue; got != "3m0s" {
		t.Fatalf("flag default %q, want the preset", got)
	}
}

// A secret file that disappears between connects yields an empty secret, and
// the failure is logged once per distinct error rather than on every attempt.
// The log names the path and never the old secret.
func TestNatsSecretFileReadFailureLogsOnce(t *testing.T) {
	var output bytes.Buffer
	previous := log
	log = funcr.New(func(prefix, args string) { output.WriteString(args + "\n") }, funcr.Options{})
	t.Cleanup(func() { log = previous })

	path := writeSecret(t, "token", "first-token\n")
	secret := &secretFile{path: path}
	if got := secret.read(); got != "first-token" {
		t.Fatalf("read %q", got)
	}

	path = path + ".gone"
	secret.path = path
	for range 3 {
		if got := secret.read(); got != "" {
			t.Fatalf("an unreadable secret read as %q", got)
		}
	}
	if logged := strings.Count(output.String(), "unable to read NATS secret file"); logged != 1 {
		t.Fatalf("logged %d times, want once: %s", logged, output.String())
	}
	if !strings.Contains(output.String(), path) || strings.Contains(output.String(), "first-token") {
		t.Fatalf("log = %s", output.String())
	}

	// A different failure is news and is logged again.
	secret.path = writeSecret(t, "empty", "\n")
	secret.read()
	if logged := strings.Count(output.String(), "unable to read NATS secret file"); logged != 2 {
		t.Fatalf("a new error was not logged: %s", output.String())
	}
}
