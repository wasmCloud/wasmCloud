package runtime_operator

import (
	"context"
	"fmt"
	"slices"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
)

const crdWatchInterval = 30 * time.Second

// serverResourcesLister is the part of discovery.DiscoveryInterface crdWatch uses.
type serverResourcesLister interface {
	ServerResourcesForGroupVersion(groupVersion string) (*metav1.APIResourceList, error)
}

// crdWatch waits for a CRD that was missing at startup and then fails the
// manager, so the pod restarts with that kind's controller registered.
type crdWatch struct {
	discovery serverResourcesLister
	gv        schema.GroupVersion
	kind      string
	// interval defaults to crdWatchInterval.
	interval time.Duration
}

func (w *crdWatch) installed() (bool, error) {
	resources, err := w.discovery.ServerResourcesForGroupVersion(w.gv.String())
	if apierrors.IsNotFound(err) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return slices.ContainsFunc(resources.APIResources, func(r metav1.APIResource) bool {
		return r.Kind == w.kind
	}), nil
}

func (w *crdWatch) Start(ctx context.Context) error {
	interval := w.interval
	if interval == 0 {
		interval = crdWatchInterval
	}
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
			// A discovery error is retried on the next tick.
			if ok, _ := w.installed(); ok {
				return fmt.Errorf("%s CRD is now installed, restarting to enable its controller", w.kind)
			}
		}
	}
}

// NeedLeaderElection is false so standby replicas restart too.
func (w *crdWatch) NeedLeaderElection() bool {
	return false
}
