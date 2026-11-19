package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/go-logr/logr"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// A heartbeat counts toward a settled fleet only once a reconcile has started
// the clock on it. Arrival alone, as in the moments after the operator starts,
// is not a window of continuous hearing.
func TestFleetWitness_HeardButNotYetObserved(t *testing.T) {
	var w fleetWitness
	now := time.Now()
	w.heard(now)
	if w.settled(now.Add(time.Second), testUnreachableTimeout) {
		t.Fatal("settled on a heartbeat no reconcile has observed")
	}
	w.observe(now, testUnreachableTimeout)
	w.heard(now.Add(testUnreachableTimeout))
	if !w.settled(now.Add(testUnreachableTimeout), testUnreachableTimeout) {
		t.Fatal("not settled after hearing the fleet for a whole window")
	}
}

// Finalizing a Host that cannot list its Workloads fails, so the finalizer
// stays and the Workloads are not stranded on a host that is gone.
func TestHostFinalizeListFailure(t *testing.T) {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "finalizing", Namespace: testNamespace},
		HostID:     testHeartbeatHostID,
	}
	deletes := 0
	r := &HostReconciler{
		Client: hostPodClientBuilder(t, host).WithInterceptorFuncs(interceptor.Funcs{
			List: failHostList,
			Delete: func(context.Context, client.WithWatch, client.Object, ...client.DeleteOption) error {
				deletes++
				return nil
			},
		}).Build(),
	}
	if err := r.finalize(context.Background(), host); !errors.Is(err, errAPIUnavailable) {
		t.Fatalf("finalize returned %v, want the list error", err)
	}
	if deletes != 0 {
		t.Errorf("deleted %d workloads without a list", deletes)
	}
}

type failingSubscribeBus struct {
	mockBus
	err error
}

func (b *failingSubscribeBus) Subscribe(string, int) (wasmbus.Subscription, error) { return nil, b.err }

// Without the heartbeat subscription no Host is ever marked seen, so the
// updater fails to start rather than running deaf.
func TestHostStatusUpdaterStartFailsWithoutSubscription(t *testing.T) {
	subscribeErr := errors.New("nats: connection closed")
	updater := &hostStatusUpdater{bus: &failingSubscribeBus{err: subscribeErr}}
	if err := updater.Start(context.Background()); !errors.Is(err, subscribeErr) {
		t.Fatalf("Start returned %v, want the subscribe error", err)
	}
}

func TestHeartbeatHandlerIgnoresUnusableHeartbeats(t *testing.T) {
	for name, tc := range map[string]struct {
		data  func(t *testing.T) []byte
		funcs interceptor.Funcs
		reads int
	}{
		"malformed payload": {data: func(*testing.T) []byte { return []byte("{not json") }},
		// Writing on a failed read could create a Host over one the cache
		// simply could not return.
		"host cannot be read": {
			data: func(t *testing.T) []byte { return heartbeatBytes(t, "host", "host-id", "node") },
			funcs: interceptor.Funcs{
				Get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
					return errAPIUnavailable
				},
			},
			reads: 1,
		},
	} {
		t.Run(name, func(t *testing.T) {
			writes, reads := 0, 0
			get := tc.funcs.Get
			tc.funcs.Get = func(ctx context.Context, c client.WithWatch, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
				reads++
				if get != nil {
					return get(ctx, c, key, obj, opts...)
				}
				return c.Get(ctx, key, obj, opts...)
			}
			tc.funcs.Create = func(context.Context, client.WithWatch, client.Object, ...client.CreateOption) error {
				writes++
				return nil
			}
			tc.funcs.Patch = func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
				writes++
				return nil
			}
			tc.funcs.SubResourcePatch = func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
				writes++
				return nil
			}
			fleet := &fleetWitness{}
			updater := &hostStatusUpdater{
				client: hostPodClientBuilder(t).WithInterceptorFuncs(tc.funcs).Build(), operatorNamespace: testNamespace, fleet: fleet,
			}
			updater.handleHeartbeat(context.Background(), logr.Discard(), "runtime.operator.heartbeat.host-id", tc.data(t))
			if writes != 0 || reads != tc.reads {
				t.Errorf("reads=%d writes=%d for an unusable heartbeat, want reads=%d and no writes", reads, writes, tc.reads)
			}
		})
	}
}
