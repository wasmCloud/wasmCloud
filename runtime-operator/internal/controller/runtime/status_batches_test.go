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

const batchHost = "batch-host"

func TestBatchStatusReusesObservationsAndFallsBackOnIncompleteReply(t *testing.T) {
	objects := make([]client.Object, 0, 8)
	for i := 0; len(objects) < cap(objects); i++ {
		id := fmt.Sprintf("replica-%d", i)
		if statusBucket(batchHost, id) != statusBucket(batchHost, "replica-0") {
			continue
		}
		w := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{Name: id}}
		w.Status.HostID, w.Status.WorkloadID = batchHost, id
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
	// A host already seen answering gets full batches rather than a probe.
	s := &statusBatches{supported: map[string]time.Time{batchHost: time.Now()}}
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
	if s.cached(batchHost, "replica-0", time.Now().Add(time.Second), true) != nil {
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

func TestBatchStatusReusesOnlySettledObservations(t *testing.T) {
	const settlingID = "settling-replica"
	w := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{Name: settlingID}}
	w.Status.HostID, w.Status.WorkloadID = batchHost, settlingID
	c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(w).
		WithIndex(&runtimev1alpha1.Workload{}, statusBucketIndex, statusBucketValue).Build()
	state := runtimev2.WorkloadState_WORKLOAD_STATE_STARTING
	bus := &mockBus{respond: func(msg *wasmbus.Message) (*wasmbus.Message, error) {
		data, err := protojson.Marshal(&runtimev2.WorkloadStatusBatchResponse{Workloads: []*runtimev2.WorkloadStatus{
			{WorkloadId: settlingID, WorkloadState: state},
		}})
		return &wasmbus.Message{Data: data}, err
	}}
	r := &WorkloadReconciler{Client: c, Bus: bus}
	s := &statusBatches{}
	if got := s.status(t.Context(), r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_STARTING {
		t.Fatalf("first observation: %v", got)
	}
	state = runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING
	// Within statusUnsettledTTL the batch still answers its bucket.
	if got := s.status(t.Context(), r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_STARTING || len(bus.requests) != 1 {
		t.Fatalf("fresh starting observation not reused: %v after %d requests", got, len(bus.requests))
	}
	entry := s.entries["batch-host/"+settlingID]
	entry.at = entry.at.Add(-2 * statusUnsettledTTL)
	s.entries["batch-host/"+settlingID] = entry
	if got := s.status(t.Context(), r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING {
		t.Fatalf("starting observation was reused past its ttl: %v", got)
	}
	if got := s.status(t.Context(), r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING || len(bus.requests) != 2 {
		t.Fatalf("running observation was not reused: %v after %d requests", got, len(bus.requests))
	}
	// The first batch to an unproven host is a short probe; once it answered,
	// later batches get the full status timeout.
	if probe, full := time.Until(bus.deadlines[0]), time.Until(bus.deadlines[1]); probe > statusBatchProbeTimeout || full <= statusBatchProbeTimeout {
		t.Fatalf("probe deadline %v, supported deadline %v", probe, full)
	}
}
