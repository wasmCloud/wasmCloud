package main

import (
	"context"
	"slices"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	gatewayWorkloadFinalizerName = "runtime.wasmcloud.dev/gateway-workload-finalizer"
	gatewayHostFinalizerName     = "runtime.wasmcloud.dev/gateway-host-finalizer"
)

// dropGatewayFinalizer removes a gateway finalizer from obj if it carries one.
//
// The gateway's routing tables live only in the process that built them, so
// there is nothing to release once that process is gone. A gateway finalizer
// outlives it, though: a cluster whose gateway has been scaled down, removed,
// or replaced is left with objects that can never finish deleting, so strip the
// finalizer wherever it turns up.
func dropGatewayFinalizer(ctx context.Context, c client.Client, obj client.Object, finalizer string) error {
	if !controllerutil.ContainsFinalizer(obj, finalizer) {
		return nil
	}

	base := obj.DeepCopyObject().(client.Object)
	controllerutil.RemoveFinalizer(obj, finalizer)
	// A merge patch replaces the finalizer list wholesale, so patch under
	// optimistic lock: without it a stale copy would silently restore
	// finalizers the operator has since removed, or drop ones it has added.
	return c.Patch(ctx, obj, client.MergeFromWithOptions(base, client.MergeFromWithOptimisticLock{}))
}

// WorkloadReconciler
type WorkloadReconciler struct {
	client.Client
	Registry WorkloadRegistry
}

func (a *WorkloadReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := ctrl.LoggerFrom(ctx).WithValues("workload", req.NamespacedName)

	workload := &runtimev1alpha1.Workload{}
	if err := a.Get(ctx, req.NamespacedName, workload); err != nil {
		if apierrors.IsNotFound(err) {
			// A deletion can reach the reconciler after the object is gone, so
			// the registry indexes routes by object key: that is all this has
			// left to identify them by. Skipping it would keep proxying to the
			// workload until the gateway restarts.
			return ctrl.Result{}, a.Registry.DeregisterWorkload(ctx, req.NamespacedName)
		}
		return ctrl.Result{}, err
	}

	if err := dropGatewayFinalizer(ctx, a.Client, workload, gatewayWorkloadFinalizerName); err != nil {
		return ctrl.Result{}, err
	}

	if workload.DeletionTimestamp != nil {
		return ctrl.Result{}, a.Registry.DeregisterWorkload(ctx, req.NamespacedName)
	}

	// workload hasn't been placed, do nothing
	if !workload.Status.AllTrue(runtimev1alpha1.WorkloadConditionPlacement) {
		return ctrl.Result{}, nil
	}

	hostname := workloadHostname(workload)

	// no hostname configured, nothing to route
	if hostname == "" {
		return ctrl.Result{}, a.Registry.DeregisterWorkload(ctx, req.NamespacedName)
	}

	log.Info("Reconciling Workload")

	if workload.Status.IsAvailable() {
		if err := a.Registry.RegisterWorkload(ctx, req.NamespacedName, workload.Status.HostID, workload.Status.WorkloadID, hostname); err != nil {
			log.Error(err, "failed to register workload")
			return ctrl.Result{}, err
		}
	} else {
		if err := a.Registry.DeregisterWorkload(ctx, req.NamespacedName); err != nil {
			log.Error(err, "failed to deregister workload")
			return ctrl.Result{}, err
		}
	}

	return ctrl.Result{}, nil
}

func workloadHostname(workload *runtimev1alpha1.Workload) string {
	for _, iface := range workload.Spec.HostInterfaces {
		if iface.Namespace != "wasi" || iface.Package != "http" {
			continue
		}
		// A component's HTTP entrypoint is advertised as either the p2
		// `incoming-handler` or the p3 `handler` interface; the host serves both
		// (wash-runtime's is_incoming_http_handler accepts either), so the gateway
		// must register a route for either. Without the p3 case, a workload that
		// exports only wasi:http/handler@0.3.0 reaches Ready but has no gateway
		// route and every request to it 503s.
		if !slices.Contains(iface.Interfaces, "incoming-handler") &&
			!slices.Contains(iface.Interfaces, "handler") {
			continue
		}
		if h, ok := iface.Config["host"]; ok {
			return h
		}
	}
	return ""
}

func (a *WorkloadReconciler) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	return ctrl.
		NewControllerManagedBy(manager).
		For(&runtimev1alpha1.Workload{}).
		Complete(a)
}

// HostReconciler
type HostReconciler struct {
	client.Client
	Registry HostRegistry
}

func (a *HostReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := ctrl.LoggerFrom(ctx).WithValues("host", req.NamespacedName)

	host := &runtimev1alpha1.Host{}
	if err := a.Get(ctx, req.NamespacedName, host); err != nil {
		if apierrors.IsNotFound(err) {
			// See the Workload reconciler: the object is gone, so its key is
			// all that identifies what it registered.
			return ctrl.Result{}, a.Registry.DeregisterHost(ctx, req.NamespacedName)
		}
		return ctrl.Result{}, err
	}

	if err := dropGatewayFinalizer(ctx, a.Client, host, gatewayHostFinalizerName); err != nil {
		return ctrl.Result{}, err
	}

	if host.DeletionTimestamp != nil {
		return ctrl.Result{}, a.Registry.DeregisterHost(ctx, req.NamespacedName)
	}

	log.Info("Reconciling Host")

	if host.Status.IsAvailable() {
		if err := a.Registry.RegisterHost(ctx, req.NamespacedName, host.HostID, host.Hostname, int(host.HTTPPort)); err != nil {
			log.Error(err, "failed to register host")
			return ctrl.Result{}, err
		}
	} else {
		if err := a.Registry.DeregisterHost(ctx, req.NamespacedName); err != nil {
			log.Error(err, "failed to deregister host")
			return ctrl.Result{}, err
		}
	}

	return ctrl.Result{}, nil
}

func (a *HostReconciler) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	return ctrl.
		NewControllerManagedBy(manager).
		For(&runtimev1alpha1.Host{}).
		Complete(a)
}
