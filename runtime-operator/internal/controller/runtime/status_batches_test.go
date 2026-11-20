package runtime

import (
	"context"
	"errors"
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
		got := batchStatus(t, s, r, w)
		if got == nil || got.WorkloadId != w.Status.WorkloadID {
			t.Fatal("missing or mismatched observation")
		}
	}
	if len(bus.requests) != 1 {
		t.Fatalf("8 observations required %d RPCs", len(bus.requests))
	}
	if s.cached(batchHost, "replica-0", time.Now().Add(time.Second), time.Now().Add(-time.Minute)) != nil {
		t.Fatal("observation predating placement was reused")
	}
	s = &statusBatches{}
	bus.respond = nil
	bus.reply = &wasmbus.Message{Data: []byte(`{"workloads":[]}`)}
	for _, obj := range objects {
		if batchStatus(t, s, r, obj.(*runtimev1alpha1.Workload)) != nil {
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
	if got := batchStatus(t, s, r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_STARTING {
		t.Fatalf("first observation: %v", got)
	}
	state = runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING
	// Within statusUnsettledTTL the batch still answers its bucket.
	if got := batchStatus(t, s, r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_STARTING || len(bus.requests) != 1 {
		t.Fatalf("fresh starting observation not reused: %v after %d requests", got, len(bus.requests))
	}
	entry := s.entries["batch-host/"+settlingID]
	entry.at = entry.at.Add(-2 * statusUnsettledTTL)
	s.entries["batch-host/"+settlingID] = entry
	if got := batchStatus(t, s, r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING {
		t.Fatalf("starting observation was reused past its ttl: %v", got)
	}
	if got := batchStatus(t, s, r, w); got.GetWorkloadState() != runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING || len(bus.requests) != 2 {
		t.Fatalf("running observation was not reused: %v after %d requests", got, len(bus.requests))
	}
	// The first batch to an unproven host is a short probe; once it answered,
	// later batches get the full status timeout.
	if probe, full := time.Until(bus.deadlines[0]), time.Until(bus.deadlines[1]); probe > statusBatchProbeTimeout || full <= statusBatchProbeTimeout {
		t.Fatalf("probe deadline %v, supported deadline %v", probe, full)
	}
}

func TestStatusFetchServesOnlyTheEntriesItRefreshed(t *testing.T) {
	fetched := time.Now()
	starting := &runtimev2.WorkloadStatus{WorkloadState: runtimev2.WorkloadState_WORKLOAD_STATE_STARTING}
	s := &statusBatches{entries: map[string]statusObservation{
		"host/old":   {status: starting, at: fetched.Add(-20 * time.Second)},
		"host/fresh": {status: starting, at: fetched},
	}}
	if s.cached("host", "old", time.Time{}, fetched) != nil {
		t.Fatal("an unsettled status the fetch did not refresh outlived its ttl")
	}
	if s.cached("host", "fresh", time.Time{}, fetched) == nil {
		t.Fatal("the status the fetch just made was not returned")
	}
	if s.cached("host", "old", time.Time{}, time.Time{}) != nil {
		t.Fatal("a failed fetch returned an expired unsettled status")
	}
}

func batchStatus(t *testing.T, s *statusBatches, r *WorkloadReconciler, w *runtimev1alpha1.Workload) *runtimev2.WorkloadStatus {
	t.Helper()
	status, err := s.status(t.Context(), r, w)
	if err != nil {
		t.Fatal(err)
	}
	return status
}

func batchWorkload(id string) *runtimev1alpha1.Workload {
	w := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{Name: "w-" + fmt.Sprint(len(id)) + "-" + id[:min(len(id), 40)]}}
	w.Status.HostID, w.Status.WorkloadID = batchHost, id
	return w
}

func answerBatch(t *testing.T, sizes *[]int) func(*wasmbus.Message) (*wasmbus.Message, error) {
	return func(msg *wasmbus.Message) (*wasmbus.Message, error) {
		var req runtimev2.WorkloadStatusBatchRequest
		if err := protojson.Unmarshal(msg.Data, &req); err != nil {
			return nil, err
		}
		*sizes = append(*sizes, len(req.WorkloadIds))
		response := &runtimev2.WorkloadStatusBatchResponse{}
		for _, id := range req.WorkloadIds {
			if len(id) > statusBatchMaxIDLen {
				t.Errorf("batch carried a %d-byte ID", len(id))
			}
			response.Workloads = append(response.Workloads, &runtimev2.WorkloadStatus{WorkloadId: id, WorkloadState: runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING})
		}
		data, err := protojson.Marshal(response)
		return &wasmbus.Message{Data: data}, err
	}
}

func TestStatusBatchCoversLargeBucketsAndSkipsOversizedIDs(t *testing.T) {
	target := statusBucket(batchHost, "replica-0")
	objects := []client.Object{}
	for i := 0; len(objects) < 300; i++ {
		id := fmt.Sprintf("replica-%d", i)
		if statusBucket(batchHost, id) == target {
			objects = append(objects, batchWorkload(id))
		}
	}
	long := ""
	for i := 0; ; i++ {
		long = fmt.Sprintf("%0129d", i)
		if statusBucket(batchHost, long) == target {
			break
		}
	}
	oversized := batchWorkload(long)
	objects = append(objects, oversized)
	c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(objects...).
		WithIndex(&runtimev1alpha1.Workload{}, statusBucketIndex, statusBucketValue).Build()
	var sizes []int
	bus := &mockBus{respond: answerBatch(t, &sizes)}
	r := &WorkloadReconciler{Client: c, Bus: bus}
	s := &statusBatches{supported: map[string]time.Time{batchHost: time.Now()}}
	if got := batchStatus(t, s, r, objects[0].(*runtimev1alpha1.Workload)); got == nil {
		t.Fatal("no status from the batch")
	}
	if len(sizes) != 3 || sizes[0]+sizes[1]+sizes[2] != 300 {
		t.Fatalf("bucket of 300 batched as %v", sizes)
	}
	// Every other Workload in the bucket is answered by that one fetch.
	for _, obj := range objects[1:300] {
		if batchStatus(t, s, r, obj.(*runtimev1alpha1.Workload)) == nil {
			t.Fatal("bucket member outside the first chunk was not covered")
		}
	}
	if len(sizes) != 3 {
		t.Fatalf("covered members fetched again: %v", sizes)
	}
	if batchStatus(t, s, r, oversized) != nil || len(sizes) != 3 {
		t.Fatal("an ID past the host's limit went into a batch")
	}
}

func TestSilentProvenHostFailsSyncWithoutAnotherTimeout(t *testing.T) {
	w := batchWorkload("replica-0")
	c := fake.NewClientBuilder().WithScheme(newScaleTestScheme(t)).WithObjects(w).
		WithIndex(&runtimev1alpha1.Workload{}, statusBucketIndex, statusBucketValue).Build()
	r := &WorkloadReconciler{Client: c, Bus: &mockBus{err: context.DeadlineExceeded}}
	s := &statusBatches{supported: map[string]time.Time{batchHost: time.Now()}}
	if _, err := s.status(t.Context(), r, w); !errors.Is(err, errHostSilent) {
		t.Fatalf("silent proven host: %v", err)
	}
	// One silent batch unproves the host. A rolled-back host that no longer
	// serves batches is probed again and falls back rather than failing
	// every Sync until the support TTL lapses.
	if got, err := s.status(t.Context(), r, w); got != nil || err != nil {
		t.Fatalf("silent host was not probed again: %v %v", got, err)
	}
	if !time.Now().Before(s.unsupported[batchHost]) {
		t.Fatal("a failed re-probe did not mark the host unsupported")
	}
	// An unproven host that does not answer is an older host: fall back.
	s = &statusBatches{}
	if got, err := s.status(t.Context(), r, w); got != nil || err != nil {
		t.Fatalf("unanswered probe: %v %v", got, err)
	}
}

func TestStatusObservationWithinPlacementSecondIsDistrusted(t *testing.T) {
	placed := time.Now().Truncate(time.Second)
	s := &statusBatches{entries: map[string]statusObservation{
		"host/id": {status: &runtimev2.WorkloadStatus{}, at: placed.Add(300 * time.Millisecond)},
	}}
	if s.cached("host", "id", placed, time.Time{}) != nil {
		t.Fatal("an observation that may predate the start was used")
	}
}
