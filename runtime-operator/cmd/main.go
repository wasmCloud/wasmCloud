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
	"errors"
	"flag"
	"net/http"
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
		hostNamespaces              string
		allowSharedHosts            bool
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
		"Comma-separated list of namespaces to watch for WorkloadDeployment-side resources "+
			"(artifacts, workloads, workloadreplicasets, workloaddeployments + the "+
			"services/endpointslices/configmaps/secrets/events the workload reconcilers touch). "+
			"If empty, watches all namespaces.",
	)
	flag.StringVar(
		&hostNamespaces,
		"host-namespaces",
		"",
		"Comma-separated list of namespaces where host Pods run. The operator's Pod informer "+
			"cache and per-namespace Pod RBAC cover this set so HostPodReconciler can manage "+
			"finalizers on host Pods. Does NOT affect where Host CRDs are created — every Host "+
			"always lives in the operator's own namespace. If empty, host Pods are assumed to "+
			"run only in the operator's own namespace.",
	)
	flag.BoolVar(
		&allowSharedHosts,
		"allow-shared-hosts",
		true,
		"If true (default), a WorkloadDeployment may schedule onto a Host whose Environment "+
			"differs from the workload's own namespace via spec.template.spec.environment. "+
			"If false, scheduling is locked to the workload's own namespace and any non-matching "+
			"environment is rejected.",
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

	// OPERATOR_NAMESPACE is required: every Host CRD is created here, and
	// the namespaced Role for Host CRUD binds to this namespace.
	operatorNamespace := os.Getenv("OPERATOR_NAMESPACE")
	if operatorNamespace == "" {
		setupLog.Error(errors.New("OPERATOR_NAMESPACE is unset"), "missing required configuration")
		os.Exit(1)
	}

	operatorCfg := runtime_operator.EmbeddedOperatorConfig{
		DisableArtifactController: disableArtifactController,
		NatsURL:                   natsUrl,
		HeartbeatTTL:              60 * time.Second,
		HostCPUThreshold:          cpuBackpressureThreshold,
		HostMemoryThreshold:       memoryBackpressureThreshold,
		Namespace:                 operatorNamespace,
		HostNamespaces:            splitCSVList(hostNamespaces),
		AllowSharedHosts:          allowSharedHosts,
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

	// Surface NATS connection state.
	operatorCfg.NatsOptions = append(operatorCfg.NatsOptions,
		nats.DisconnectErrHandler(func(_ *nats.Conn, err error) {
			setupLog.Error(err, "nats disconnected")
		}),
		nats.ReconnectHandler(func(nc *nats.Conn) {
			setupLog.Info("nats reconnected", "url", nc.ConnectedUrl())
		}),
		nats.ClosedHandler(func(_ *nats.Conn) {
			setupLog.Error(nil, "nats connection closed")
		}),
	)

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

	// -watch-namespaces narrows the cache for workload-side resources
	// (artifacts, workloads, workloaddeployments, etc.). Empty == all
	// namespaces.
	cacheOpts.DefaultNamespaces = parseNamespaceSet(watchNamespaces)

	// Host objects always live in the operator's own namespace —
	// hostStatusUpdater unconditionally creates them there, regardless of
	// where the underlying host pod runs. The Host informer cache scopes
	// to that single namespace.
	hostCacheNamespaces := map[string]cache.Config{
		operatorCfg.Namespace: {},
	}

	// The Pod informer cache covers the operator's own namespace plus
	// `-host-namespaces` so HostPodReconciler can manage finalizers on
	// host Pods regardless of which namespace the platform team deploys
	// them into. The HostPodLabel predicate keeps the working set
	// bounded to actual host Pods.
	podCacheNamespaces := map[string]cache.Config{
		operatorCfg.Namespace: {},
	}
	for _, ns := range operatorCfg.HostNamespaces {
		podCacheNamespaces[ns] = cache.Config{}
	}
	if len(podCacheNamespaces) == 0 {
		podCacheNamespaces[cache.AllNamespaces] = cache.Config{}
	}

	cacheOpts.ByObject = map[client.Object]cache.ByObject{
		&runtimev1alpha1.Host{}: {Namespaces: hostCacheNamespaces},
		&corev1.Pod{}:           {Namespaces: podCacheNamespaces},
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

	embeddedOperator, err := runtime_operator.NewEmbeddedOperator(ctx, mgr, operatorCfg)
	if err != nil {
		setupLog.Error(err, "unable to create runtime operator")
		os.Exit(1)
	}

	// +kubebuilder:scaffold:builder

	if err := mgr.AddHealthzCheck("healthz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up health check")
		os.Exit(1)
	}
	// Liveness must reflect NATS connectivity: a permanently closed connection
	// means the operator can no longer observe host heartbeats, so the kubelet
	// should restart the pod. Only the terminal closed state fails the probe —
	// transient reconnecting stays healthy so routine NATS rollouts don't
	// trigger restart storms.
	if err := mgr.AddHealthzCheck("nats", natsLivenessCheck(embeddedOperator.NatsConn)); err != nil {
		setupLog.Error(err, "unable to set up nats health check")
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

// natsLivenessCheck reports the operator as unhealthy only when the NATS
// connection is permanently closed. A closed connection means the operator can
// no longer observe host heartbeats, so Kubernetes should restart the pod. It
// deliberately stays healthy while merely reconnecting so routine NATS rollouts
// don't trigger restart storms.
func natsLivenessCheck(nc *nats.Conn) healthz.Checker {
	return func(_ *http.Request) error {
		if nc.IsClosed() {
			return errors.New("nats connection permanently closed")
		}
		return nil
	}
}

// splitCSVList parses a comma-separated string into a trimmed, non-empty
// slice of values. Returns nil for an empty input.
func splitCSVList(raw string) []string {
	if raw == "" {
		return nil
	}
	parts := strings.Split(raw, ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if p = strings.TrimSpace(p); p != "" {
			out = append(out, p)
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// parseNamespaceSet parses a comma-separated namespace list into a
// controller-runtime cache namespace map. Returns nil for an empty input,
// which controller-runtime interprets as "all namespaces".
func parseNamespaceSet(raw string) map[string]cache.Config {
	if raw == "" {
		return nil
	}
	parts := strings.Split(raw, ",")
	out := make(map[string]cache.Config, len(parts))
	for _, ns := range parts {
		ns = strings.TrimSpace(ns)
		if ns != "" {
			out[ns] = cache.Config{}
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}
