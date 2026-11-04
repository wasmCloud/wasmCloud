package main

import (
	"errors"
	"flag"
	"os"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
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
	options, err := parseGatewayOptions(os.Args[1:], os.Stderr)
	if errors.Is(err, flag.ErrHelp) {
		return
	}
	if err != nil {
		os.Exit(2)
	}
	ctrl.SetLogger(zap.New(zap.UseFlagOptions(&options.logging)))
	if err := runGateway(ctrl.SetupSignalHandler(), options, defaultGatewayDependencies()); err != nil {
		setupLog.Error(err, "gateway stopped")
		os.Exit(1)
	}
}
