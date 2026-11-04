package runtime

import (
	"context"
	"reflect"
	"testing"

	"github.com/go-logr/logr"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	heartbeatVictimName   = "victim"
	heartbeatHostResource = "hosts"
)

func TestHeartbeatHandler_RejectsSubjectMismatch(t *testing.T) {
	for _, tc := range []struct{ subject, id string }{
		{"runtime.operator.heartbeat.attacker", heartbeatVictimName},
		{"runtime.operator.heartbeat.", ""},
		{"runtime.operator.heartbeat.a.b", "a.b"},
		{"runtime.operator.heartbeat.*", "*"},
		{"runtime.operator.heartbeat.>", ">"},
		{"runtime.operator.heartbeat.a b", "a b"},
		{"other.victim", heartbeatVictimName},
	} {
		t.Run(tc.subject, func(t *testing.T) {
			// Invalid subjects must return before fleet or Kubernetes access.
			updater := &hostStatusUpdater{}
			updater.handleHeartbeat(context.Background(), logr.Discard(), tc.subject, heartbeatBytes(t, heartbeatVictimName, tc.id, "node"))
		})
	}
}

func TestHeartbeatHandler_RejectsNameTakeover(t *testing.T) {
	for _, staleRead := range []bool{false, true} {
		victim := &runtimev1alpha1.Host{
			ObjectMeta: metav1.ObjectMeta{Name: heartbeatVictimName, Namespace: testNamespace},
			HostID:     "victim-id", Hostname: "victim-node",
		}
		reads, writes := 0, 0
		c := hostPodClientBuilder(t, victim).WithStatusSubresource(&runtimev1alpha1.Host{}).WithInterceptorFuncs(interceptor.Funcs{
			Get: func(ctx context.Context, c client.WithWatch, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
				reads++
				if staleRead {
					return apierrors.NewNotFound(schema.GroupResource{Group: runtimev1alpha1.GroupVersion.Group, Resource: heartbeatHostResource}, key.Name)
				}
				return c.Get(ctx, key, obj, opts...)
			},
			Patch: func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
				writes++
				return nil
			},
			SubResourcePatch: func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
				writes++
				return nil
			},
		}).Build()
		before := victim.DeepCopy()
		updater := &hostStatusUpdater{client: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}}
		updater.handleHeartbeat(context.Background(), logr.Discard(), "runtime.operator.heartbeat.attacker", heartbeatBytes(t, heartbeatVictimName, "attacker", "attacker-node"))
		if reads != 1 || writes != 0 {
			t.Fatalf("staleRead=%v: reads=%d writes=%d", staleRead, reads, writes)
		}
		hosts := &runtimev1alpha1.HostList{}
		if err := c.List(context.Background(), hosts); err != nil {
			t.Fatal(err)
		}
		if len(hosts.Items) != 1 || hosts.Items[0].HostID != before.HostID || hosts.Items[0].Hostname != before.Hostname || !reflect.DeepEqual(hosts.Items[0].Status, before.Status) {
			t.Fatalf("staleRead=%v: victim changed: %+v", staleRead, hosts.Items)
		}
	}
}

func TestHeartbeatHandler_RejectsConcurrentReplacement(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "heartbeat-replacement")
	const subject = "runtime.operator.heartbeat.original"
	const name = "host"
	updater := &hostStatusUpdater{client: c, operatorNamespace: ns, fleet: &fleetWitness{}}
	updater.handleHeartbeat(ctx, logr.Discard(), subject, heartbeatBytes(t, name, "original", "node"))
	for _, hostname := range []string{"node", "changed-node"} {
		original := &runtimev1alpha1.Host{}
		if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, original); err != nil {
			t.Fatal(err)
		}
		original.HostID = "original"
		if err := c.Update(ctx, original); err != nil {
			t.Fatal(err)
		}
		raced := &heartbeatReadClient{Client: c, afterGet: func(obj client.Object) error {
			replacement := obj.(*runtimev1alpha1.Host).DeepCopy()
			replacement.HostID = "replacement"
			return c.Update(ctx, replacement)
		}}
		updater.client = raced
		updater.handleHeartbeat(ctx, logr.Discard(), subject, heartbeatBytes(t, name, "original", hostname))
		got := &runtimev1alpha1.Host{}
		if err := c.Get(ctx, client.ObjectKeyFromObject(original), got); err != nil {
			t.Fatal(err)
		}
		if got.HostID != "replacement" || got.Hostname != original.Hostname || !got.Status.LastSeen.Equal(&original.Status.LastSeen) {
			t.Fatalf("concurrent replacement overwritten: %+v", got)
		}
	}
}

type heartbeatReadClient struct {
	client.Client
	afterGet func(client.Object) error
}

func (c *heartbeatReadClient) Get(ctx context.Context, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
	if err := c.Client.Get(ctx, key, obj, opts...); err != nil {
		return err
	}
	return c.afterGet(obj)
}
