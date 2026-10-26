// Package natsclient configures how a NATS client identifies itself from
// command-line flags.
package natsclient

import (
	"flag"
	"fmt"
	"os"
	"strings"
	"sync"

	"github.com/nats-io/nats.go"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
)

const defaultPrefix = "nats"

var log = logf.Log.WithName("natsclient")

// Flags is how a NATS client identifies itself. Register binds each field to a
// flag; a field set beforehand becomes that flag's default.
type Flags struct {
	Creds        string
	Nkey         string
	TokenFile    string
	User         string
	PasswordFile string
	Name         string
	InboxPrefix  string

	prefix string
}

// Register defines the flags on fs as -<prefix>-creds, -<prefix>-nkey and so
// on, such as -nats-creds for the prefix "nats".
func (f *Flags) Register(fs *flag.FlagSet, prefix string) {
	f.prefix = prefix
	fs.StringVar(&f.Creds, f.flag("creds"), f.Creds, "Path to NATS credentials file.")
	fs.StringVar(&f.Nkey, f.flag("nkey"), f.Nkey, "Path to a file holding a NATS nkey seed.")
	fs.StringVar(&f.TokenFile, f.flag("token-file"), f.TokenFile,
		"Path to a file holding a NATS auth token, read again on every connect.")
	fs.StringVar(&f.User, f.flag("user"), f.User,
		fmt.Sprintf("NATS user name. Requires -%s.", f.flag("password-file")))
	fs.StringVar(&f.PasswordFile, f.flag("password-file"), f.PasswordFile,
		fmt.Sprintf("Path to a file holding the password for -%s, read again on every connect.", f.flag("user")))
	fs.StringVar(&f.Name, f.flag("name"), f.Name, "Connection name presented to NATS.")
	fs.StringVar(&f.InboxPrefix, f.flag("inbox-prefix"), f.InboxPrefix,
		"Subject prefix for the replies to this client's requests. Empty uses the NATS default, _INBOX. "+
			"Set one only this client may subscribe to, so other NATS clients can neither read nor forge "+
			"its replies.")
}

// flag is the name of one of the flags, without its leading dash.
func (f Flags) flag(name string) string {
	prefix := f.prefix
	if prefix == "" {
		prefix = defaultPrefix
	}
	return prefix + "-" + name
}

// Options builds the client options, keeping secrets out of process
// arguments. Credentials, tokens, and passwords are reread on connect; a new
// bare nkey requires a restart.
func (f Flags) Options() ([]nats.Option, error) {
	var opts []nats.Option

	if err := f.oneCredential(); err != nil {
		return nil, err
	}
	switch {
	case f.Creds != "":
		opts = append(opts, nats.UserCredentials(f.Creds))
	case f.Nkey != "":
		opt, err := nats.NkeyOptionFromSeed(f.Nkey)
		if err != nil {
			return nil, fmt.Errorf("-%s: %w", f.flag("nkey"), err)
		}
		opts = append(opts, opt)
	case f.TokenFile != "":
		if _, err := readSecret(f.TokenFile); err != nil {
			return nil, fmt.Errorf("-%s: %w", f.flag("token-file"), err)
		}
		token := &secretFile{path: f.TokenFile}
		opts = append(opts, nats.TokenHandler(token.read))
	case f.User != "":
		if _, err := readSecret(f.PasswordFile); err != nil {
			return nil, fmt.Errorf("-%s: %w", f.flag("password-file"), err)
		}
		password := &secretFile{path: f.PasswordFile}
		opts = append(opts, nats.UserInfoHandler(func() (string, string) {
			return f.User, password.read()
		}))
	}
	// Reloaded credentials can recover once kubelet refreshes the mounted Secret.
	if f.Creds != "" || f.TokenFile != "" || f.User != "" {
		opts = append(opts, nats.IgnoreAuthErrorAbort())
	}
	if f.Name != "" {
		opts = append(opts, nats.Name(f.Name))
	}
	if f.InboxPrefix != "" {
		if err := validInboxPrefix(f.InboxPrefix); err != nil {
			return nil, fmt.Errorf("-%s: %w", f.flag("inbox-prefix"), err)
		}
		opts = append(opts, nats.CustomInboxPrefix(f.InboxPrefix))
	}
	return opts, nil
}

// oneCredential refuses more than one credential: the NATS client sends all
// of them and the server authenticates with only one.
func (f Flags) oneCredential() error {
	switch {
	case f.PasswordFile != "" && f.User == "":
		return fmt.Errorf("-%s requires -%s", f.flag("password-file"), f.flag("user"))
	case f.User != "" && f.PasswordFile == "":
		return fmt.Errorf("-%s requires -%s", f.flag("user"), f.flag("password-file"))
	}
	var set []string
	if f.Creds != "" {
		set = append(set, "-"+f.flag("creds"))
	}
	if f.Nkey != "" {
		set = append(set, "-"+f.flag("nkey"))
	}
	if f.TokenFile != "" {
		set = append(set, "-"+f.flag("token-file"))
	}
	if f.User != "" {
		set = append(set, "-"+f.flag("user"))
	}
	if len(set) > 1 {
		return fmt.Errorf("%s each authenticate the client; set one", strings.Join(set, " and "))
	}
	return nil
}

// secretFile rereads a secret on every connect. A failed read is logged once
// per distinct error, which names the path and never the contents.
type secretFile struct {
	path    string
	mu      sync.Mutex
	lastErr string
}

func (s *secretFile) read() string {
	secret, err := readSecret(s.path)
	s.mu.Lock()
	defer s.mu.Unlock()
	if err == nil {
		s.lastErr = ""
		return secret
	}
	if err.Error() != s.lastErr {
		s.lastErr = err.Error()
		log.Error(err, "unable to read NATS secret file", "path", s.path)
	}
	return ""
}

func readSecret(path string) (string, error) {
	contents, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	// Strip one line ending; other whitespace belongs to the secret.
	secret, hasNewline := strings.CutSuffix(string(contents), "\n")
	if hasNewline {
		secret = strings.TrimSuffix(secret, "\r")
	}
	if secret == "" {
		return "", fmt.Errorf("%s is empty", path)
	}
	return secret, nil
}

// validInboxPrefix requires literal, nonempty subject tokens.
func validInboxPrefix(prefix string) error {
	if strings.ContainsAny(prefix, "*> \t\r\n") || strings.HasSuffix(prefix, ".") {
		return fmt.Errorf("%q must be a literal subject with no wildcards, spaces or trailing dot", prefix)
	}
	for token := range strings.SplitSeq(prefix, ".") {
		if token == "" {
			return fmt.Errorf("%q has an empty subject token", prefix)
		}
	}
	return nil
}
