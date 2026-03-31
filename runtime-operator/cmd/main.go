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
	"crypto/tls"
	"flag"
	"os"
	"strings"
	"time"

	// Import all Kubernetes client auth plugins (e.g. Azure, GCP, OIDC, etc.)
	// to ensure that exec-entrypoint and run can make use of them.
	_ "k8s.io/client-go/plugin/pkg/client/auth"

	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/cache"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/healthz"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
	"sigs.k8s.io/controller-runtime/pkg/metrics/filters"
	metricsserver "sigs.k8s.io/controller-runtime/pkg/metrics/server"
	"sigs.k8s.io/controller-runtime/pkg/webhook"

	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"

	"github.com/nats-io/nats.go"
	runtime_operator "go.wasmcloud.dev/runtime-operator/v2"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	// +kubebuilder:scaffold:imports
)

var (
	scheme   = runtime.NewScheme()
	setupLog = ctrl.Log.WithName("setup")
)

func init() {
	utilruntime.Must(clientgoscheme.AddToScheme(scheme))
	utilruntime.Must(runtimev1alpha1.AddToScheme(scheme))
	// +kubebuilder:scaffold:scheme
}

func main() {
	var (
		metricsAddr                 string
		natsUrl                     string
		natsCreds                   string
		natsCa                      string
		natsClientCert              string
		natsClientKey               string
		natsTLSFirst                bool
		enableLeaderElection        bool
		probeAddr                   string
		secureMetrics               bool
		enableHTTP2                 bool
		jsonLog                     bool
		cpuBackpressureThreshold    float64
		memoryBackpressureThreshold float64
		disableArtifactController   bool
		watchNamespaces             string
	)

	flag.StringVar(&metricsAddr, "metrics-bind-address", ":8081", "The address the metrics endpoint binds to. "+
		"Use :8443 for HTTPS or :8080 for HTTP, or leave as 0 to disable the metrics service.")
	flag.StringVar(&probeAddr, "health-probe-bind-address", ":8082", "The address the probe endpoint binds to.")
	flag.StringVar(&natsCreds, "nats-creds", "", "Path to NATS credentials file.")
	flag.StringVar(&natsCa, "nats-ca", "", "Path to TLS CA pem")
	flag.StringVar(&natsClientCert, "nats-client-cert", "", "Path to TLS client certificate pem")
	flag.StringVar(&natsClientKey, "nats-client-key", "", "Path to TLS client key pem")
	flag.BoolVar(&natsTLSFirst, "nats-tls-first", false, "Skip NATS Server discovery during TLS")
	flag.StringVar(&natsUrl, "nats-url", wasmbus.NatsDefaultURL, "The nats server address to connect to.")
	flag.BoolVar(&enableLeaderElection, "leader-elect", false,
		"Enable leader election for controller manager. "+
			"Enabling this will ensure there is only one active controller manager.")
	flag.BoolVar(&secureMetrics, "metrics-secure", false,
		"If set, the metrics endpoint is served securely via HTTPS. Use --metrics-secure=false to use HTTP instead.")
	flag.BoolVar(&enableHTTP2, "enable-http2", false,
		"If set, HTTP/2 will be enabled for the metrics and webhook servers")
	flag.BoolVar(&jsonLog, "json-log", false, "Output logs in JSON format")
	flag.Float64Var(&cpuBackpressureThreshold, "cpu-backpressure-threshold", 80.0, "CPU backpressure threshold (%)")
	flag.Float64Var(
		&memoryBackpressureThreshold,
		"memory-backpressure-threshold",
		80.0,
		"Memory backpressure threshold (%)")
	flag.BoolVar(
		&disableArtifactController,
		"disable-artifact-controller",
		false,
		"Delegates Artifact reconciliation to an external controller.",
	)
	flag.StringVar(
		&watchNamespaces,
		"watch-namespaces",
		"",
		"Comma-separated list of namespaces to watch. If empty, watches all namespaces.",
	)

	opts := zap.Options{
		Development: true,
	}
	opts.BindFlags(flag.CommandLine)
	flag.Parse()

	zapOpts := []zap.Opts{
		zap.UseFlagOptions(&opts),
	}
	if jsonLog {
		zapOpts = append(zapOpts, zap.JSONEncoder())
	}
	ctrl.SetLogger(zap.New(
		zapOpts...,
	))

	operatorCfg := runtime_operator.EmbeddedOperatorConfig{
		DisableArtifactController: disableArtifactController,
		NatsURL:                   natsUrl,
		HeartbeatTTL:              60 * time.Second,
		HostCPUThreshold:          cpuBackpressureThreshold,
		HostMemoryThreshold:       memoryBackpressureThreshold,
		Namespace:                 os.Getenv("OPERATOR_NAMESPACE"),
	}

	if natsCreds != "" {
		operatorCfg.NatsOptions = append(operatorCfg.NatsOptions, nats.UserCredentials(natsCreds))
	}

	if natsCa != "" {
		operatorCfg.NatsOptions = append(operatorCfg.NatsOptions, nats.RootCAs(natsCa))
	}

	if natsClientCert != "" && natsClientKey != "" {
		operatorCfg.NatsOptions = append(operatorCfg.NatsOptions, nats.ClientCert(natsClientCert, natsClientKey))
	}

	if natsTLSFirst {
		operatorCfg.NatsOptions = append(operatorCfg.NatsOptions, nats.TLSHandshakeFirst())
	}

	// if the enable-http2 flag is false (the default), http/2 should be disabled
	// due to its vulnerabilities. More specifically, disabling http/2 will
	// prevent from being vulnerable to the HTTP/2 Stream Cancellation and
	// Rapid Reset CVEs. For more information see:
	// - https://github.com/advisories/GHSA-qppj-fm5r-hxr3
	// - https://github.com/advisories/GHSA-4374-p667-p6c8
	disableHTTP2 := func(c *tls.Config) {
		setupLog.Info("disabling http/2")
		c.NextProtos = []string{"http/1.1"}
	}

	var tlsOpts []func(*tls.Config)
	if !enableHTTP2 {
		tlsOpts = append(tlsOpts, disableHTTP2)
	}

	webhookServer := webhook.NewServer(webhook.Options{
		TLSOpts: tlsOpts,
	})

	// Metrics endpoint is enabled in 'config/default/kustomization.yaml'. The Metrics options configure the server.
	// More info:
	// - https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.19.1/pkg/metrics/server
	// - https://book.kubebuilder.io/reference/metrics.html
	metricsServerOptions := metricsserver.Options{
		BindAddress:   metricsAddr,
		SecureServing: secureMetrics,
		TLSOpts:       tlsOpts,
	}

	if secureMetrics {
		// FilterProvider is used to protect the metrics endpoint with authn/authz.
		// These configurations ensure that only authorized users and service accounts
		// can access the metrics endpoint. The RBAC are configured in 'config/rbac/kustomization.yaml'. More info:
		// https://pkg.go.dev/sigs.k8s.io/controller-runtime@v0.19.1/pkg/metrics/filters#WithAuthenticationAndAuthorization
		metricsServerOptions.FilterProvider = filters.WithAuthenticationAndAuthorization

		// TODO(user): If CertDir, CertName, and KeyName are not specified, controller-runtime will automatically
		// generate self-signed certificates for the metrics server. While convenient for development and testing,
		// this setup is not recommended for production.
	}
	var cacheOpts cache.Options

	// If watch namespaces is set, only watch the specified namespaces. Otherwise, watch all namespaces.
	if watchNamespaces != "" {
		namespaces := strings.Split(watchNamespaces, ",")
		toWatchNamespaces := make(map[string]cache.Config, len(namespaces))
		for _, ns := range namespaces {
			ns = strings.TrimSpace(ns)
			if ns != "" {
				toWatchNamespaces[ns] = cache.Config{}
			}
		}
		cacheOpts.DefaultNamespaces = toWatchNamespaces
	}

	// Restrict the Pod cache to the operator's own namespace so the cache
	// only requires a namespaced Role (not a ClusterRole) for Pod list/watch.
	// ByObject overrides DefaultNamespaces for the specified type, so Pods are
	// always scoped to the operator namespace regardless of -watch-namespaces.
	if operatorCfg.Namespace != "" {
		cacheOpts.ByObject = map[client.Object]cache.ByObject{
			&corev1.Pod{}: {
				Namespaces: map[string]cache.Config{
					operatorCfg.Namespace: {},
				},
			},
		}
	}

	mgr, err := ctrl.NewManager(ctrl.GetConfigOrDie(), ctrl.Options{
		Scheme:                 scheme,
		Metrics:                metricsServerOptions,
		Cache:                  cacheOpts,
		WebhookServer:          webhookServer,
		HealthProbeBindAddress: probeAddr,
		LeaderElection:         enableLeaderElection,
		LeaderElectionID:       "d34db3ef.wasmcloud.dev",
		// LeaderElectionReleaseOnCancel defines if the leader should step down voluntarily
		// when the Manager ends. This requires the binary to immediately end when the
		// Manager is stopped, otherwise, this setting is unsafe. Setting this significantly
		// speeds up voluntary leader transitions as the new leader don't have to wait
		// LeaseDuration time first.
		//
		// In the default scaffold provided, the program ends immediately after
		// the manager stops, so would be fine to enable this option. However,
		// if you are doing or is intended to do any operation such as perform cleanups
		// after the manager stops then its usage might be unsafe.
		// LeaderElectionReleaseOnCancel: true,
	})
	if err != nil {
		setupLog.Error(err, "unable to start manager")
		os.Exit(1)
	}

	ctx := ctrl.SetupSignalHandler()

	_, err = runtime_operator.NewEmbeddedOperator(ctx, mgr, operatorCfg)
	if err != nil {
		setupLog.Error(err, "unable to create runtime operator")
		os.Exit(1)
	}

	// +kubebuilder:scaffold:builder

	if err := mgr.AddHealthzCheck("healthz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up health check")
		os.Exit(1)
	}
	if err := mgr.AddReadyzCheck("readyz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up ready check")
		os.Exit(1)
	}

	setupLog.Info("starting manager")
	if err := mgr.Start(ctx); err != nil {
		setupLog.Error(err, "problem running manager")
		os.Exit(1)
	}
}
