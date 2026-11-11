package runtime

import (
	"fmt"
	"testing"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
	"google.golang.org/protobuf/encoding/protojson"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
)

func TestBatchStatusReusesObservationsAndFallsBackOnIncompleteReply(t *testing.T) {
	objects := make([]client.Object, 0, 8)
	for i := 0; len(objects) < cap(objects); i++ {
		id := fmt.Sprintf("replica-%d", i)
		if statusBucket("batch-host", id) != statusBucket("batch-host", "replica-0") {
			continue
		}
		w := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{Name: id}}
		w.Status.HostID, w.Status.WorkloadID = "batch-host", id
		objects = append(objects, w)
	}
	c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(objects...).
		WithIndex(&runtimev1alpha1.Workload{}, statusBucketIndex, statusBucketValue).Build()
	bus := &mockBus{respond: func(msg *wasmbus.Message) (*wasmbus.Message, error) {
		var req runtimev2.WorkloadStatusBatchRequest
		if err := protojson.Unmarshal(msg.Data, &req); err != nil {
			return nil, err
		}
		if len(req.WorkloadIds) != len(objects) {
			t.Errorf("batch contains %d IDs", len(req.WorkloadIds))
		}
		response := &runtimev2.WorkloadStatusBatchResponse{}
		for _, id := range req.WorkloadIds {
			response.Workloads = append(response.Workloads, &runtimev2.WorkloadStatus{WorkloadId: id, WorkloadState: runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING})
		}
		data, err := protojson.Marshal(response)
		return &wasmbus.Message{Data: data}, err
	}}
	r := &WorkloadReconciler{Client: c, Bus: bus}
	s := &statusBatches{}
	for _, obj := range objects {
		w := obj.(*runtimev1alpha1.Workload)
		got := s.status(t.Context(), r, w)
		if got == nil || got.WorkloadId != w.Status.WorkloadID {
			t.Fatal("missing or mismatched observation")
		}
	}
	if len(bus.requests) != 1 {
		t.Fatalf("8 observations required %d RPCs", len(bus.requests))
	}
	if s.cached("batch-host", "replica-0", time.Now().Add(time.Second)) != nil {
		t.Fatal("observation predating placement was reused")
	}
	s = &statusBatches{}
	bus.respond = nil
	bus.reply = &wasmbus.Message{Data: []byte(`{"workloads":[]}`)}
	for _, obj := range objects {
		if s.status(t.Context(), r, obj.(*runtimev1alpha1.Workload)) != nil {
			t.Fatal("incomplete reply treated as a snapshot")
		}
	}
	if len(bus.requests) != 2 {
		t.Fatalf("unsupported host probed repeatedly: %d calls", len(bus.requests))
	}
}

func TestBatchStatusRejectsForeignOrMissingIDs(t *testing.T) {
	for _, statuses := range [][]*runtimev2.WorkloadStatus{nil, {nil}, {{WorkloadId: "unexpected-id"}}} {
		if validateStatusBatch([]string{"wanted"}, statuses) == nil {
			t.Fatalf("accepted %+v", statuses)
		}
	}
}
