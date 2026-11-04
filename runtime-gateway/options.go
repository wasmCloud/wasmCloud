package main

import (
	"flag"
	"fmt"
	"io"
	"net/url"
	"strings"

	"k8s.io/client-go/rest"
	"k8s.io/client-go/tools/clientcmd"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
)

type gatewayOptions struct {
	bindAddr         string
	fallbackEndpoint string
	kubeconfig       string
	logging          zap.Options
}

func parseGatewayOptions(args []string, output io.Writer) (gatewayOptions, error) {
	var options gatewayOptions
	flags := flag.NewFlagSet("runtime-gateway", flag.ContinueOnError)
	flags.SetOutput(output)
	flags.StringVar(&options.bindAddr, "bind-addr", ":8000", "Address to bind the HTTP gateway to")
	flags.StringVar(&options.fallbackEndpoint, "fallback-endpoint", "", "Fallback origin URL, such as https://notfound.example:8443")
	flags.StringVar(&options.kubeconfig, "kubeconfig", "", "Path to a kubeconfig; otherwise use KUBECONFIG or in-cluster configuration")
	flags.BoolVar(&options.logging.Development, "dev-mode", false, "Enable development mode logging")
	options.logging.BindFlags(flags)
	// Default to JSON with epoch timestamps in both logging modes.
	_ = flags.Set("zap-encoder", "json")
	_ = flags.Set("zap-time-encoding", "epoch")
	err := flags.Parse(args)
	return options, err
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
