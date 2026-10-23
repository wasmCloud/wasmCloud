package main

import (
	"errors"
	"fmt"
	"os"
	"strings"
	"sync"

	"github.com/nats-io/nats.go"
)

// natsClientFlags is how the operator identifies itself to NATS.
type natsClientFlags struct {
	creds        string
	nkey         string
	tokenFile    string
	user         string
	passwordFile string
	name         string
	inboxPrefix  string
}

// options keeps secrets out of process arguments. Credentials, tokens, and
// passwords are reread on connect; a new bare nkey requires a restart.
func (f natsClientFlags) options() ([]nats.Option, error) {
	var opts []nats.Option

	if err := f.oneCredential(); err != nil {
		return nil, err
	}
	switch {
	case f.creds != "":
		opts = append(opts, nats.UserCredentials(f.creds))
	case f.nkey != "":
		opt, err := nats.NkeyOptionFromSeed(f.nkey)
		if err != nil {
			return nil, fmt.Errorf("-nats-nkey: %w", err)
		}
		opts = append(opts, opt)
	case f.tokenFile != "":
		if _, err := readSecret(f.tokenFile); err != nil {
			return nil, fmt.Errorf("-nats-token-file: %w", err)
		}
		token := &secretFile{path: f.tokenFile}
		opts = append(opts, nats.TokenHandler(token.read))
	case f.user != "":
		if _, err := readSecret(f.passwordFile); err != nil {
			return nil, fmt.Errorf("-nats-password-file: %w", err)
		}
		password := &secretFile{path: f.passwordFile}
		opts = append(opts, nats.UserInfoHandler(func() (string, string) {
			return f.user, password.read()
		}))
	}
	// Reloaded credentials can recover once kubelet refreshes the mounted Secret.
	if f.creds != "" || f.tokenFile != "" || f.user != "" {
		opts = append(opts, nats.IgnoreAuthErrorAbort())
	}
	if f.name != "" {
		opts = append(opts, nats.Name(f.name))
	}
	if f.inboxPrefix != "" {
		if err := validInboxPrefix(f.inboxPrefix); err != nil {
			return nil, fmt.Errorf("-nats-inbox-prefix: %w", err)
		}
		opts = append(opts, nats.CustomInboxPrefix(f.inboxPrefix))
	}
	return opts, nil
}

// oneCredential refuses more than one credential: the NATS client sends all
// of them and the server authenticates with only one.
func (f natsClientFlags) oneCredential() error {
	switch {
	case f.passwordFile != "" && f.user == "":
		return errors.New("-nats-password-file requires -nats-user")
	case f.user != "" && f.passwordFile == "":
		return errors.New("-nats-user requires -nats-password-file")
	}
	var set []string
	if f.creds != "" {
		set = append(set, "-nats-creds")
	}
	if f.nkey != "" {
		set = append(set, "-nats-nkey")
	}
	if f.tokenFile != "" {
		set = append(set, "-nats-token-file")
	}
	if f.user != "" {
		set = append(set, "-nats-user")
	}
	if len(set) > 1 {
		return fmt.Errorf("%s each authenticate the operator; set one", strings.Join(set, " and "))
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
		setupLog.Error(err, "unable to read NATS secret file", "path", s.path)
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
