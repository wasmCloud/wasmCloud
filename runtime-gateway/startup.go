package main

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httputil"
	"time"

	"k8s.io/client-go/rest"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/healthz"
)

type gatewayDependencies struct {
	getConfig     func(string) (*rest.Config, error)
	newManager    func(*rest.Config, ctrl.Options) (ctrl.Manager, error)
	setupHost     func(*HostReconciler, context.Context, ctrl.Manager) error
	setupWorkload func(*WorkloadReconciler, context.Context, ctrl.Manager) error
}

func defaultGatewayDependencies() gatewayDependencies {
	return gatewayDependencies{
		getConfig: loadGatewayConfig, newManager: ctrl.NewManager,
		setupHost: (*HostReconciler).SetupWithManager, setupWorkload: (*WorkloadReconciler).SetupWithManager,
	}
}

func runGateway(ctx context.Context, options gatewayOptions, deps gatewayDependencies) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	var fallback Fallback
	if options.fallbackEndpoint != "" {
		external, err := externalFallback(options.fallbackEndpoint)
		if err != nil {
			return err
		}
		fallback = external
	}
	kubeConfig, err := deps.getConfig(options.kubeconfig)
	if err != nil {
		return fmt.Errorf("get kubeconfig: %w", err)
	}
	mgr, err := deps.newManager(kubeConfig, ctrl.Options{Scheme: scheme})
	if err != nil {
		return fmt.Errorf("create manager: %w", err)
	}
	if err := mgr.AddHealthzCheck("healthz", healthz.Ping); err != nil {
		return fmt.Errorf("set up health check: %w", err)
	}
	if err := mgr.AddReadyzCheck("readyz", healthz.Ping); err != nil {
		return fmt.Errorf("set up readiness check: %w", err)
	}
	if fallback == nil {
		internal, err := newFallbackServer("127.0.0.1:0")
		if err != nil {
			return fmt.Errorf("listen for fallback requests: %w", err)
		}
		defer func() { _ = internal.Close() }()
		if err := internal.SetupWithManager(ctx, mgr); err != nil {
			return fmt.Errorf("register fallback server: %w", err)
		}
		fallback = internal
	}
	tracker := newHostTracker(fallback)
	gateway := &HTTPGateway{
		BindAddr: options.bindAddr, Resolver: tracker,
		Proxy: &httputil.ReverseProxy{Transport: &http.Transport{
			MaxIdleConns: 0, MaxIdleConnsPerHost: 100, IdleConnTimeout: 90 * time.Second,
		}},
	}
	if err := gateway.SetupWithManager(ctx, mgr); err != nil {
		return fmt.Errorf("register HTTP gateway: %w", err)
	}
	if err := deps.setupHost(&HostReconciler{Client: mgr.GetClient(), Registry: tracker}, ctx, mgr); err != nil {
		return fmt.Errorf("set up host reconciler: %w", err)
	}
	if err := deps.setupWorkload(&WorkloadReconciler{Client: mgr.GetClient(), Registry: tracker}, ctx, mgr); err != nil {
		return fmt.Errorf("set up workload reconciler: %w", err)
	}
	ctrl.LoggerFrom(ctx).Info("starting gateway manager")
	if err := mgr.Start(ctx); err != nil {
		return fmt.Errorf("start manager: %w", err)
	}
	return nil
}
