package main

import (
	"context"
	"slices"

	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/api/runtime/v1alpha1"
)

// WorkloadReconciler
type WorkloadReconciler struct {
	client.Client
	Registry WorkloadRegistry
}

func (a *WorkloadReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	log := ctrl.LoggerFrom(ctx).WithValues("workload", req.NamespacedName)

	workload := &runtimev1alpha1.Workload{}
	err := a.Get(ctx, req.NamespacedName, workload)
	if err != nil {
		if client.IgnoreNotFound(err) == nil {
			return ctrl.Result{}, nil
		}
		return ctrl.Result{}, err
	}

	// workload hasn't been placed, do nothing
	if !workload.Status.AllTrue(runtimev1alpha1.WorkloadConditionPlacement) {
		return ctrl.Result{}, nil
	}

	var hostname string

	for _, iface := range workload.Spec.HostInterfaces {
		if iface.Namespace == "wasi" && iface.Package == "http" && slices.Contains(iface.Interfaces, "incoming-handler") {
			if h, ok := iface.Config["host"]; ok {
				hostname = h
				break
			}
		}
	}

	// no hostname configured, nothing to register
	if hostname == "" {
		return ctrl.Result{}, nil
	}

	log.Info("Reconciling Workload")

	if workload.DeletionTimestamp != nil {
		if err := a.Registry.DeregisterWorkload(ctx, workload.Status.HostID, workload.Status.WorkloadID, hostname); err != nil {
			log.Error(err, "failed to deregister workload")
			return ctrl.Result{}, err
		}
		return ctrl.Result{}, nil
	}

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
	err := a.Get(ctx, req.NamespacedName, host)
	if err != nil {
		if client.IgnoreNotFound(err) == nil {
			return ctrl.Result{}, nil
		}
		return ctrl.Result{}, err
	}

	log.Info("Reconciling Host")

	if host.DeletionTimestamp != nil {
		if err := a.Registry.DeregisterHost(ctx, host.HostID); err != nil {
			log.Error(err, "failed to deregister host")
			return ctrl.Result{}, err
		}
		return ctrl.Result{}, nil
	}

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
