package main

import (
	"errors"
	"fmt"
	"os"
	"strings"

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

	if f.creds != "" && f.nkey != "" {
		return nil, errors.New("-nats-creds and -nats-nkey both sign the server nonce; set one")
	}
	if f.creds != "" {
		opts = append(opts, nats.UserCredentials(f.creds))
	}
	if f.nkey != "" {
		opt, err := nats.NkeyOptionFromSeed(f.nkey)
		if err != nil {
			return nil, fmt.Errorf("-nats-nkey: %w", err)
		}
		opts = append(opts, opt)
	}
	if f.tokenFile != "" {
		if _, err := readSecret(f.tokenFile); err != nil {
			return nil, fmt.Errorf("-nats-token-file: %w", err)
		}
		opts = append(opts, nats.TokenHandler(func() string {
			// A missing file leaves no token to present.
			token, _ := readSecret(f.tokenFile)
			return token
		}))
	}
	switch {
	case f.passwordFile != "" && f.user == "":
		return nil, errors.New("-nats-password-file requires -nats-user")
	case f.user != "":
		if f.passwordFile != "" {
			if _, err := readSecret(f.passwordFile); err != nil {
				return nil, fmt.Errorf("-nats-password-file: %w", err)
			}
		}
		opts = append(opts, nats.UserInfoHandler(func() (string, string) {
			if f.passwordFile == "" {
				return f.user, ""
			}
			password, _ := readSecret(f.passwordFile)
			return f.user, password
		}))
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
