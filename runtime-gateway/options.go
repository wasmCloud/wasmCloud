package main

import (
	"flag"
	"fmt"
	"io"
	"log/slog"
	"net/url"
	"strings"

	"k8s.io/client-go/rest"
	"k8s.io/client-go/tools/clientcmd"
	ctrl "sigs.k8s.io/controller-runtime"
)

type gatewayOptions struct {
	bindAddr         string
	fallbackEndpoint string
	kubeconfig       string
	logging          gatewayLoggingOptions
}

func parseGatewayOptions(args []string, output io.Writer) (gatewayOptions, error) {
	var options gatewayOptions
	flags := flag.NewFlagSet("runtime-gateway", flag.ContinueOnError)
	flags.SetOutput(output)
	flags.StringVar(&options.bindAddr, "bind-addr", ":8000", "Address to bind the HTTP gateway to")
	flags.StringVar(&options.fallbackEndpoint, "fallback-endpoint", "", "Fallback origin URL, such as https://notfound.example:8443")
	flags.StringVar(&options.kubeconfig, "kubeconfig", "", "Path to a kubeconfig; otherwise use KUBECONFIG or in-cluster configuration")
	var development, explicitLevel bool
	flags.BoolVar(&development, "dev-mode", false, "Enable debug logging")
	flags.Func("log-level", "Log level: debug, info (default), warn, or error; overrides dev-mode", func(value string) error {
		if err := options.logging.level.UnmarshalText([]byte(value)); err != nil {
			return err
		}
		switch options.logging.level {
		case slog.LevelDebug, slog.LevelInfo, slog.LevelWarn, slog.LevelError:
		default:
			return fmt.Errorf("invalid log level %q: use debug, info, warn, or error", value)
		}
		explicitLevel = true
		return nil
	})
	flags.StringVar(&options.logging.format, "log-format", "json", "Log format: json or text")
	flags.StringVar(&options.logging.timeEncoding, "log-time-encoding", "epoch", "Timestamp encoding: epoch, rfc3339, or rfc3339nano")
	if err := flags.Parse(args); err != nil {
		return options, err
	}
	if development && !explicitLevel {
		options.logging.level = slog.LevelDebug
	}
	if err := options.logging.validate(); err != nil {
		_, _ = fmt.Fprintln(output, err)
		return options, err
	}
	return options, nil
}

func loadGatewayConfig(path string) (*rest.Config, error) {
	if path == "" {
		return ctrl.GetConfig()
	}
	config, err := clientcmd.BuildConfigFromFlags("", path)
	if err != nil {
		return nil, err
	}
	// Disable client-side rate limiting, as controller-runtime does.
	config.QPS = -1
	return config, nil
}

func externalFallback(endpoint string) (*ExternalFallback, error) {
	u, err := url.Parse(endpoint)
	if err != nil {
		return nil, fmt.Errorf("parse fallback endpoint: %w", err)
	}
	scheme := strings.ToLower(u.Scheme)
	if (scheme != "http" && scheme != "https") || u.Hostname() == "" {
		return nil, fmt.Errorf("fallback endpoint must be an absolute HTTP or HTTPS URL with a host")
	}
	if u.User != nil || (u.Path != "" && u.Path != "/") || u.RawQuery != "" || u.ForceQuery || u.Fragment != "" {
		return nil, fmt.Errorf("fallback endpoint must be an origin without credentials, a path, a query, or a fragment")
	}
	return &ExternalFallback{Scheme: scheme, Endpoint: u.Host}, nil
}
