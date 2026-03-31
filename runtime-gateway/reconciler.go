package main

import (
	"context"
	"slices"

	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	gatewayWorkloadFinalizerName = "runtime.wasmcloud.dev/gateway-workload-finalizer"
	gatewayHostFinalizerName     = "runtime.wasmcloud.dev/gateway-host-finalizer"
)

// WorkloadReconciler
type WorkloadReconciler struct {
	client.Client
	Registry WorkloadRegistry
}

func (a *WorkloadReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := ctrl.LoggerFrom(ctx).WithValues("workload", req.NamespacedName)

	workload := &runtimev1alpha1.Workload{}
	if err := a.Get(ctx, req.NamespacedName, workload); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Handle deletion: run cleanup and remove our finalizer before etcd removes
	// the object. Without this, a deleted object may disappear from the cache
	// before the reconciler runs, leaving stale HostTracker entries.
	if workload.DeletionTimestamp != nil {
		if controllerutil.ContainsFinalizer(workload, gatewayWorkloadFinalizerName) {
			if err := a.deregisterWorkload(ctx, workload); err != nil {
				log.Error(err, "failed to deregister workload on deletion")
				return ctrl.Result{}, err
			}
			base := workload.DeepCopy()
			controllerutil.RemoveFinalizer(workload, gatewayWorkloadFinalizerName)
			return ctrl.Result{}, a.Patch(ctx, workload, client.MergeFrom(base))
		}
		return ctrl.Result{}, nil
	}

	// workload hasn't been placed, do nothing
	if !workload.Status.AllTrue(runtimev1alpha1.WorkloadConditionPlacement) {
		return ctrl.Result{}, nil
	}

	hostname := workloadHostname(workload)

	// no hostname configured, nothing to register
	if hostname == "" {
		return ctrl.Result{}, nil
	}

	// Ensure our finalizer is present so we can deregister on deletion.
	if !controllerutil.ContainsFinalizer(workload, gatewayWorkloadFinalizerName) {
		base := workload.DeepCopy()
		controllerutil.AddFinalizer(workload, gatewayWorkloadFinalizerName)
		return ctrl.Result{}, a.Patch(ctx, workload, client.MergeFrom(base))
	}

	log.Info("Reconciling Workload")

	if workload.Status.IsAvailable() {
		if err := a.Registry.RegisterWorkload(ctx, workload.Status.HostID, workload.Status.WorkloadID, hostname); err != nil {
			log.Error(err, "failed to register workload")
			return ctrl.Result{}, err
		}
	} else {
		if err := a.Registry.DeregisterWorkload(ctx, workload.Status.HostID, workload.Status.WorkloadID, hostname); err != nil {
			log.Error(err, "failed to deregister workload")
			return ctrl.Result{}, err
		}
	}

	return ctrl.Result{}, nil
}

func (a *WorkloadReconciler) deregisterWorkload(ctx context.Context, workload *runtimev1alpha1.Workload) error {
	hostname := workloadHostname(workload)
	if hostname == "" {
		return nil
	}
	return a.Registry.DeregisterWorkload(ctx, workload.Status.HostID, workload.Status.WorkloadID, hostname)
}

func workloadHostname(workload *runtimev1alpha1.Workload) string {
	for _, iface := range workload.Spec.HostInterfaces {
		if iface.Namespace == "wasi" && iface.Package == "http" && slices.Contains(iface.Interfaces, "incoming-handler") {
			if h, ok := iface.Config["host"]; ok {
				return h
			}
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
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	// Handle deletion: deregister before etcd removes the object. Without this,
	// the host may vanish from the cache before we can clean up HostTracker.
	if host.DeletionTimestamp != nil {
		if controllerutil.ContainsFinalizer(host, gatewayHostFinalizerName) {
			if err := a.Registry.DeregisterHost(ctx, host.HostID); err != nil {
				log.Error(err, "failed to deregister host on deletion")
				return ctrl.Result{}, err
			}
			base := host.DeepCopy()
			controllerutil.RemoveFinalizer(host, gatewayHostFinalizerName)
			return ctrl.Result{}, a.Patch(ctx, host, client.MergeFrom(base))
		}
		return ctrl.Result{}, nil
	}

	// Ensure our finalizer is present so we can deregister on deletion.
	if !controllerutil.ContainsFinalizer(host, gatewayHostFinalizerName) {
		base := host.DeepCopy()
		controllerutil.AddFinalizer(host, gatewayHostFinalizerName)
		return ctrl.Result{}, a.Patch(ctx, host, client.MergeFrom(base))
	}

	log.Info("Reconciling Host")

	if host.Status.IsAvailable() {
		if err := a.Registry.RegisterHost(ctx, host.HostID, host.Hostname, int(host.HTTPPort)); err != nil {
			log.Error(err, "failed to register host")
			return ctrl.Result{}, err
		}
	} else {
		if err := a.Registry.DeregisterHost(ctx, host.HostID); err != nil {
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
