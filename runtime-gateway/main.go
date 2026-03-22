package main

import (
	"flag"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"time"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"

	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/healthz"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/api/runtime/v1alpha1"
)

var (
	scheme   = runtime.NewScheme()
	setupLog = ctrl.Log.WithName("setup")
)

func init() {
	utilruntime.Must(corev1.AddToScheme(scheme))
	utilruntime.Must(appsv1.AddToScheme(scheme))
	utilruntime.Must(runtimev1alpha1.AddToScheme(scheme))
}

func main() {
	var (
		devMode          bool
		bindAddr         string
		fallbackEndpoint string
	)

	flag.BoolVar(&devMode, "dev-mode", false, "Enable development mode logging")
	flag.StringVar(&bindAddr, "bind-addr", ":8000", "Address to bind the HTTP gateway to")
	flag.StringVar(&fallbackEndpoint, "fallback-endpoint", "", "Proxy Requests are routed to this endpoint when no workloads are found. Example: 'http://notfound.svc.cluster.local")
	flag.Parse()

	opts := zap.Options{
		Development: devMode,
	}
	opts.BindFlags(flag.CommandLine)

	zapOpts := []zap.Opts{
		zap.UseFlagOptions(&opts),
		zap.JSONEncoder(),
	}

	ctrl.SetLogger(zap.New(
		zapOpts...,
	))

	kubeConfig, err := ctrl.GetConfig()
	if err != nil {
		setupLog.Error(err, "could not get kubeconfig")
		os.Exit(1)
	}

	ctx := ctrl.SetupSignalHandler()

	manager, err := ctrl.NewManager(kubeConfig, ctrl.Options{
		Scheme: scheme,
	})
	if err != nil {
		setupLog.Error(err, "could not create manager")
		os.Exit(1)
	}

	if err := manager.AddHealthzCheck("healthz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up health check")
		os.Exit(1)
	}
	if err := manager.AddReadyzCheck("readyz", healthz.Ping); err != nil {
		setupLog.Error(err, "unable to set up ready check")
		os.Exit(1)
	}

	var fallback Fallback
	if fallbackEndpoint == "" {
		internalFallback := &FallbackServer{
			BindAddr: "127.0.0.1:0",
		}
		if err := internalFallback.SetupWithManager(ctx, manager); err != nil {
			setupLog.Error(err, "could not add FallbackServer to manager")
			os.Exit(1)
		}
		fallback = internalFallback
	} else {
		fallbackURL, err := url.Parse(fallbackEndpoint)
		if err != nil {
			setupLog.Error(err, "could not parse fallback endpoint URL")
			os.Exit(1)
		}
		externalFallback := &ExternalFallback{
			Scheme:   fallbackURL.Scheme,
			Endpoint: fallbackURL.Host,
		}
		fallback = externalFallback
	}

	tracker := &HostTracker{
		Fallback: fallback,
	}
	if err := tracker.SetupWithManager(ctx, manager); err != nil {
		setupLog.Error(err, "could not add HostTracker to manager")
		os.Exit(1)
	}

	httpGateway := &HTTPGateway{
		BindAddr: bindAddr,
		Resolver: tracker,
		Proxy: &httputil.ReverseProxy{
			Transport: &http.Transport{
				MaxIdleConns:        0,
				MaxIdleConnsPerHost: 100,
				IdleConnTimeout:     90 * time.Second,
			},
		},
	}
	if err := httpGateway.SetupWithManager(ctx, manager); err != nil {
		setupLog.Error(err, "could not add HTTPGateway to manager")
		os.Exit(1)
	}

	hostReconciler := &HostReconciler{
		Client:   manager.GetClient(),
		Registry: tracker,
	}
	if err := hostReconciler.SetupWithManager(ctx, manager); err != nil {
		setupLog.Error(err, "could not setup Host reconciler")
		os.Exit(1)
	}

	workloadReconciler := &WorkloadReconciler{
		Client:   manager.GetClient(),
		Registry: tracker,
	}
	if err := workloadReconciler.SetupWithManager(ctx, manager); err != nil {
		setupLog.Error(err, "could not setup Workload reconciler")
		os.Exit(1)
	}

	setupLog.Info("Starting manager")
	if err := manager.Start(ctx); err != nil {
		setupLog.Error(err, "could not start manager")
		os.Exit(1)
	}
}
