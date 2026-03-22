package runtime_operator

// This will be moved to go.wasmcloud.dev/runtime-operator/operator.go

import (
	"context"
	"time"

	"github.com/nats-io/nats.go"
	runtime_controllers "go.wasmcloud.dev/runtime-operator/internal/controller/runtime"
	"go.wasmcloud.dev/runtime-operator/pkg/wasmbus"
	"sigs.k8s.io/controller-runtime/pkg/manager"
)

type EmbeddedOperatorConfig struct {
	// NATS connection string. Used to communicate with hosts.
	NatsURL string
	// NATS options. Used to configure the NATS connection.
	NatsOptions []nats.Option
	// Heartbeat TTL. Used to determine how long to wait before considering a host unreachable.
	HeartbeatTTL time.Duration
	// Host CPU threshold (percentage).
	// Used to calculate workload scheduling, avoiding hosts that are over this threshold.
	HostCPUThreshold float64
	// Host Memory threshold (percentage).
	// Used to calculate workload scheduling, avoiding hosts that are over this threshold.
	HostMemoryThreshold float64
	// Disable Artifact Controller. If set, Artifacts must be marked as 'Ready' elsewhere.
	// Useful when introducing a custom artifact management solution.
	DisableArtifactController bool
}

// EmbeddedOperator is the main struct for the embedded operator.
// It allows embedding the Runtime Operator into other applications.
type EmbeddedOperator struct {
	Bus      wasmbus.Bus
	NatsConn *nats.Conn
}

// NewEmbeddedOperator creates a new EmbeddedOperator.
func NewEmbeddedOperator(
	ctx context.Context,
	mgr manager.Manager,
	cfg EmbeddedOperatorConfig,
) (*EmbeddedOperator, error) {
	nc, err := wasmbus.NatsConnect(cfg.NatsURL, cfg.NatsOptions...)
	if err != nil {
		return nil, err
	}
	bus := wasmbus.NewNatsBus(nc)

	if !cfg.DisableArtifactController {
		if err = (&runtime_controllers.ArtifactReconciler{
			Client: mgr.GetClient(),
			Scheme: mgr.GetScheme(),
		}).SetupWithManager(mgr); err != nil {
			return nil, err
		}
	}

	if err = (&runtime_controllers.HostReconciler{
		Client:             mgr.GetClient(),
		Scheme:             mgr.GetScheme(),
		Bus:                bus,
		UnreachableTimeout: cfg.HeartbeatTTL,
		CPUThreshold:       cfg.HostCPUThreshold,
		MemoryThreshold:    cfg.HostMemoryThreshold,
	}).SetupWithManager(mgr); err != nil {
		return nil, err
	}

	if err = (&runtime_controllers.WorkloadReconciler{
		Client: mgr.GetClient(),
		Scheme: mgr.GetScheme(),
		Bus:    bus,
	}).SetupWithManager(mgr); err != nil {
		return nil, err
	}

	if err = (&runtime_controllers.WorkloadReplicaSetReconciler{
		Client: mgr.GetClient(),
		Scheme: mgr.GetScheme(),
	}).SetupWithManager(mgr); err != nil {
		return nil, err
	}

	if err = (&runtime_controllers.WorkloadDeploymentReconciler{
		Client: mgr.GetClient(),
		Scheme: mgr.GetScheme(),
	}).SetupWithManager(mgr); err != nil {
		return nil, err
	}

	return &EmbeddedOperator{
		Bus:      bus,
		NatsConn: nc,
	}, nil
}
