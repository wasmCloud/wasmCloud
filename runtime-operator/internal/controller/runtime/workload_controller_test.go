package runtime

import (
	"context"
	"errors"
	"testing"

	"google.golang.org/protobuf/encoding/protojson"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// TestPlacementCarriesComponentInstanceLimits checks that every instance limit
// a component declares on the CRD reaches the host.
//
// Placement is the one place they are copied from the Kubernetes type to the
// proto one, field by field, and a limit that never arrives looks exactly like
// a limit that arrived and was honoured: the default for each is the
// conservative value, so an unset maxConcurrency and a dropped one both give
// one call at a time. The runtime pins the half below this (see
// `wire_limits_reach_the_runtime` in crates/wash-runtime); this pins the half
// above it, without needing a cluster.
func TestPlacementCarriesComponentInstanceLimits(t *testing.T) {
	reply, err := protojson.Marshal(&runtimev2.WorkloadStartResponse{
		WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: "workload-1"},
	})
	if err != nil {
		t.Fatalf("marshal start response: %v", err)
	}

	bus := &mockBus{reply: &wasmbus.Message{Data: reply}}
	r := &WorkloadReconciler{Bus: bus}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "limits", Namespace: "default"},
		Spec: runtimev1alpha1.WorkloadSpec{
			Components: []runtimev1alpha1.WorkloadComponent{{
				Name:                 "pooled",
				Image:                "example.com/pooled:1",
				PoolSize:             4,
				MaxInvocations:       100,
				MaxConcurrency:       8,
				ReclaimWindowSeconds: 30,
				ReclaimMinInstances:  2,
			}},
		},
		Status: runtimev1alpha1.WorkloadStatus{HostID: "host-1"},
	}

	// Placement ends by skipping the rest of the reconciliation, having sent
	// the start request this test reads back.
	if err := r.reconcilePlacement(context.Background(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatalf("reconcilePlacement: %v", err)
	}

	var req runtimev2.WorkloadStartRequest
	if err := protojson.Unmarshal(bus.gotData, &req); err != nil {
		t.Fatalf("unmarshal start request: %v", err)
	}
	components := req.GetWorkload().GetWitWorld().GetComponents()
	if len(components) != 1 {
		t.Fatalf("got %d components, want 1", len(components))
	}

	got := components[0]
	for _, limit := range []struct {
		name string
		got  int32
		want int32
	}{
		{"poolSize", got.GetPoolSize(), 4},
		{"maxInvocations", got.GetMaxInvocations(), 100},
		{"maxConcurrency", got.GetMaxConcurrency(), 8},
		{"reclaimWindowSeconds", got.GetReclaimWindowSeconds(), 30},
		{"reclaimMinInstances", got.GetReclaimMinInstances(), 2},
	} {
		if limit.got != limit.want {
			t.Errorf("%s reached the host as %d, want %d", limit.name, limit.got, limit.want)
		}
	}
}
