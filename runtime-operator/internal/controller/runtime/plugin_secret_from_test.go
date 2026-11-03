package runtime

import (
	"context"
	"errors"
	"maps"
	"strings"
	"testing"

	"google.golang.org/protobuf/encoding/protojson"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// pluginSecretWorkload asks for two plugin secrets on one host interface, one
// whole entry and one with a key renamed, beside a plain config value.
func pluginSecretWorkload() *runtimev1alpha1.Workload {
	return &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "vault-reader", Namespace: "billing"},
		Spec: runtimev1alpha1.WorkloadSpec{
			Components: []runtimev1alpha1.WorkloadComponent{{Name: "app", Image: "example.com/app:1"}},
			HostInterfaces: []runtimev1alpha1.HostInterface{{
				ConfigLayer: runtimev1alpha1.ConfigLayer{Config: map[string]string{"region": "eu"}},
				PluginSecretFrom: []runtimev1alpha1.PluginSecretReference{
					{Name: "billing-db"},
					{Name: "billing-api", Keys: []runtimev1alpha1.PluginSecretKey{
						{Key: "token", As: "api-token"},
						{Key: "endpoint"},
					}},
				},
				Namespace:  "wasi",
				Package:    "config",
				Interfaces: []string{"store"},
				Version:    "0.2.0-rc.1",
			}},
		},
		Status: runtimev1alpha1.WorkloadStatus{HostID: testHeartbeatHostID},
	}
}

func hostWithFeatures(id string, features ...string) *runtimev1alpha1.Host {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: id, Namespace: testNamespace},
		HostID:     id,
	}
	host.Status.Features = features
	host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	return host
}

func startReply(t *testing.T) *wasmbus.Message {
	t.Helper()
	reply, err := protojson.Marshal(&runtimev2.WorkloadStartResponse{
		WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: "workload-1"},
	})
	if err != nil {
		t.Fatalf("marshal start response: %v", err)
	}
	return &wasmbus.Message{Data: reply}
}

// The operator carries a reference to the host as names only: it resolves
// nothing, and no value is in the start request.
func TestPlacementPassesPluginSecretRefsThrough(t *testing.T) {
	bus := &mockBus{reply: startReply(t)}
	host := hostWithFeatures(testHeartbeatHostID, runtimev1alpha1.HostFeaturePluginSecretRefs)
	r := &WorkloadReconciler{Client: newWorkloadClient(t, host), Bus: bus, OperatorNamespace: testNamespace}

	if err := r.reconcilePlacement(context.Background(), pluginSecretWorkload()); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatalf("reconcilePlacement: %v", err)
	}

	var req runtimev2.WorkloadStartRequest
	if err := protojson.Unmarshal(bus.gotData, &req); err != nil {
		t.Fatalf("unmarshal start request: %v", err)
	}
	interfaces := req.GetWorkload().GetWitWorld().GetHostInterfaces()
	if len(interfaces) != 1 {
		t.Fatalf("got %d host interfaces, want 1", len(interfaces))
	}
	iface := interfaces[0]
	if want := map[string]string{"region": "eu"}; !maps.Equal(iface.GetConfig(), want) {
		t.Errorf("config = %v, want %v and nothing a secret supplies", iface.GetConfig(), want)
	}
	refs := iface.GetPluginSecrets()
	if len(refs) != 2 {
		t.Fatalf("got %d plugin secret refs, want 2", len(refs))
	}
	if refs[0].GetName() != "billing-db" || len(refs[0].GetKeys()) != 0 {
		t.Errorf("whole-entry ref = %v, want billing-db with no keys", refs[0])
	}
	wantKeys := map[string]string{"api-token": "token", "endpoint": "endpoint"}
	if refs[1].GetName() != "billing-api" || !maps.Equal(refs[1].GetKeys(), wantKeys) {
		t.Errorf("selected ref = %v, want billing-api with keys %v", refs[1], wantKeys)
	}
}

// An older host ignores the references and would start the Workload without
// its secrets, so nothing is sent to it. A chosen host is given up so another
// can be selected; a pinned one is kept.
func TestPlacementRefusesHostWithoutPluginSecretRefs(t *testing.T) {
	for _, tc := range []struct {
		name       string
		pinned     string
		wantHostID string
	}{
		{name: "chosen", wantHostID: ""},
		{name: "pinned", pinned: testHeartbeatHostID, wantHostID: testHeartbeatHostID},
	} {
		t.Run(tc.name, func(t *testing.T) {
			bus := &mockBus{reply: startReply(t)}
			host := hostWithFeatures(testHeartbeatHostID)
			r := &WorkloadReconciler{Client: newWorkloadClient(t, host), Bus: bus, OperatorNamespace: testNamespace}
			workload := pluginSecretWorkload()
			workload.Spec.HostID = tc.pinned

			err := r.reconcilePlacement(context.Background(), workload)
			if err == nil || !strings.Contains(err.Error(), runtimev1alpha1.HostFeaturePluginSecretRefs) {
				t.Fatalf("reconcilePlacement = %v, want a refusal naming the missing feature", err)
			}
			if bus.gotSubject != "" {
				t.Errorf("sent %q to a host without the feature", bus.gotSubject)
			}
			if workload.Status.HostID != tc.wantHostID {
				t.Errorf("HostID = %q, want %q", workload.Status.HostID, tc.wantHostID)
			}
		})
	}
}

// A pinned host that has not registered has reported no features yet.
func TestPlacementWaitsForPinnedHostFeatures(t *testing.T) {
	bus := &mockBus{reply: startReply(t)}
	r := &WorkloadReconciler{Client: newWorkloadClient(t), Bus: bus, OperatorNamespace: testNamespace}
	workload := pluginSecretWorkload()
	workload.Spec.HostID = testHeartbeatHostID

	if err := r.reconcilePlacement(context.Background(), workload); err == nil {
		t.Fatalf("reconcilePlacement placed on a host that reported no features")
	}
	if bus.gotSubject != "" {
		t.Errorf("sent %q before the pinned host reported its features", bus.gotSubject)
	}
}

func TestFindFreeHostRequiresPluginSecretRefs(t *testing.T) {
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	reconciler := func(hosts ...*runtimev1alpha1.Host) *WorkloadReconciler {
		builder := fake.NewClientBuilder().
			WithScheme(s).
			WithIndex(&runtimev1alpha1.Host{}, workloadSchedulableHostsIndex, schedulableHostIndexValue)
		for _, host := range hosts {
			builder = builder.WithObjects(host)
		}
		return &WorkloadReconciler{Client: builder.Build(), OperatorNamespace: testNamespace, AllowSharedHosts: true}
	}
	older := hostWithFeatures("older")
	newer := hostWithFeatures("newer", runtimev1alpha1.HostFeaturePluginSecretRefs)

	r := reconciler(older, newer)
	// findFreeHost shuffles its candidates, so one pick could miss the bug.
	for range 20 {
		host, err := r.findFreeHost(context.Background(), pluginSecretWorkload())
		if err != nil {
			t.Fatalf("findFreeHost: %v", err)
		}
		if host.HostID != newer.HostID {
			t.Fatalf("placed on host %q, want %q", host.HostID, newer.HostID)
		}
	}

	_, err := reconciler(older).findFreeHost(context.Background(), pluginSecretWorkload())
	if err == nil || !strings.Contains(err.Error(), runtimev1alpha1.HostFeaturePluginSecretRefs) {
		t.Errorf("findFreeHost = %v, want a refusal naming the missing feature", err)
	}

	plain := pluginSecretWorkload()
	plain.Spec.HostInterfaces[0].PluginSecretFrom = nil
	if _, err := reconciler(older).findFreeHost(context.Background(), plain); err != nil {
		t.Errorf("a Workload without plugin secrets was refused an older host: %v", err)
	}
}

func TestPluginSecretRefsRefuseDuplicateConfigKey(t *testing.T) {
	hi := runtimev1alpha1.HostInterface{
		PluginSecretFrom: []runtimev1alpha1.PluginSecretReference{{
			Name: "billing-api",
			Keys: []runtimev1alpha1.PluginSecretKey{{Key: "token"}, {Key: "other", As: "token"}},
		}},
	}
	if _, err := pluginSecretRefs(hi); err == nil {
		t.Fatalf("pluginSecretRefs accepted two keys delivered as `token`")
	}
}
