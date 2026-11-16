package runtime

import (
	"context"
	"fmt"
	"hash/fnv"
	"slices"
	"sync"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"golang.org/x/sync/singleflight"
	"sigs.k8s.io/controller-runtime/pkg/client"
)

const (
	statusBucketIndex = "workload.status.host.bucket"
	// statusBatchProbeTimeout bounds a batch to a host not yet seen answering
	// one. Older hosts never reply to the subject, so without it every probe
	// would hold a reconcile worker for the full workloadStatusTimeout.
	statusBatchProbeTimeout = time.Second
	// statusBatchSupportTTL is how long a host that answered a batch keeps the
	// full timeout without answering another.
	statusBatchSupportTTL = 10 * time.Minute
	// statusUnsettledTTL is how long a STARTING or ERROR observation answers
	// other Syncs in its bucket. Long enough that one batch serves a bucket of
	// Workloads starting together, short enough not to delay their Ready.
	statusUnsettledTTL = time.Second
)

type statusObservation struct {
	status *runtimev2.WorkloadStatus
	at     time.Time
}

// Snapshots are request/reply observations, so they need no event sequence or
// replay log. Expiry and placement-time checks prevent reuse across starts.
type statusBatches struct {
	mu          sync.Mutex
	entries     map[string]statusObservation
	unsupported map[string]time.Time
	supported   map[string]time.Time
	swept       time.Time
	requests    singleflight.Group
}

func statusBucket(host, id string) string {
	h := fnv.New32a()
	_, _ = h.Write([]byte(id))
	return fmt.Sprintf("%s/%d", host, h.Sum32()%16)
}

func statusBucketValue(obj client.Object) []string {
	w := obj.(*runtimev1alpha1.Workload)
	if w.Status.HostID == "" || w.Status.WorkloadID == "" || !w.DeletionTimestamp.IsZero() {
		return nil
	}
	return []string{statusBucket(w.Status.HostID, w.Status.WorkloadID)}
}

// ttl is how long an observation may answer later Syncs. A settled state
// lasts the reconcile interval; any other state is kept briefly, since reading
// a stale STARTING or ERROR answer would hold Sync back after the host moved on.
func ttl(status *runtimev2.WorkloadStatus) time.Duration {
	switch status.GetWorkloadState() {
	case runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING, runtimev2.WorkloadState_WORKLOAD_STATE_COMPLETED:
		return workloadReconcileInterval
	default:
		return statusUnsettledTTL
	}
}

// cached returns an observation made since placement that is still within
// its ttl. With fresh set, as for the caller that just waited on the fetch that
// produced it, any unexpired observation is returned whatever its state.
func (s *statusBatches) cached(host, id string, placed time.Time, fresh bool) *runtimev2.WorkloadStatus {
	s.mu.Lock()
	defer s.mu.Unlock()
	entry := s.entries[host+"/"+id]
	if entry.at.Before(placed) || time.Since(entry.at) >= workloadReconcileInterval {
		return nil
	}
	if !fresh && time.Since(entry.at) >= ttl(entry.status) {
		return nil
	}
	return entry.status
}

func (s *statusBatches) status(ctx context.Context, r *WorkloadReconciler, w *runtimev1alpha1.Workload) *runtimev2.WorkloadStatus {
	host, id := w.Status.HostID, w.Status.WorkloadID
	placed := w.Status.GetCondition(runtimev1alpha1.WorkloadConditionPlacement).LastTransitionTime.Time
	if cached := s.cached(host, id, placed, false); cached != nil {
		return cached
	}
	s.mu.Lock()
	unsupported := time.Now().Before(s.unsupported[host])
	s.mu.Unlock()
	if unsupported {
		return nil
	}
	key := statusBucket(host, id)
	// DoChan lets a cancelled reconcile leave without blocking on another caller.
	result := s.requests.DoChan(key, func() (any, error) {
		if cached := s.cached(host, id, placed, false); cached != nil {
			return nil, nil
		}
		return nil, s.fetch(ctx, r, host, id, key)
	})
	select {
	case <-ctx.Done():
		return nil
	case <-result:
		return s.cached(host, id, placed, true)
	}
}

func (s *statusBatches) fetch(ctx context.Context, r *WorkloadReconciler, host, id, key string) error {
	s.mu.Lock()
	proven := time.Since(s.supported[host]) < statusBatchSupportTTL
	s.mu.Unlock()
	if !proven {
		// A host not yet seen answering is probed with this one ID and a short
		// timeout: older hosts never reply, and a loaded host answers a single
		// lookup quickly, so a slow full batch cannot disable batching for it.
		return s.query(ctx, r, host, []string{id}, statusBatchProbeTimeout)
	}
	list := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, list, client.MatchingFields{statusBucketIndex: key}); err != nil {
		return err
	}
	ids := make([]string, 0, len(list.Items)+1)
	ids = append(ids, id)
	for _, w := range list.Items {
		ids = append(ids, w.Status.WorkloadID)
	}
	slices.Sort(ids)
	ids = slices.Compact(ids)
	position, _ := slices.BinarySearch(ids, id)
	start := position / 128 * 128
	return s.query(ctx, r, host, ids[start:min(start+128, len(ids))], workloadStatusTimeout)
}

func (s *statusBatches) query(ctx context.Context, r *WorkloadReconciler, host string, ids []string, timeout time.Duration) error {
	at := time.Now()
	queryCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	var response runtimev2.WorkloadStatusBatchResponse
	err := RoundTrip(queryCtx, r.Bus, NewWashHostClient(r.Bus, host).subject("workload.status.batch"),
		&runtimev2.WorkloadStatusBatchRequest{WorkloadIds: ids}, &response)
	if err == nil {
		err = validateStatusBatch(ids, response.Workloads)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.entries == nil {
		s.entries = make(map[string]statusObservation)
	}
	if s.unsupported == nil {
		s.unsupported = make(map[string]time.Time)
	}
	if s.supported == nil {
		s.supported = make(map[string]time.Time)
	}
	if time.Since(s.swept) >= workloadReconcileInterval {
		for k, v := range s.entries {
			if time.Since(v.at) >= workloadReconcileInterval {
				delete(s.entries, k)
			}
		}
		for k, until := range s.unsupported {
			if time.Now().After(until) {
				delete(s.unsupported, k)
			}
		}
		for k, seen := range s.supported {
			if time.Since(seen) >= statusBatchSupportTTL {
				delete(s.supported, k)
			}
		}
		s.swept = time.Now()
	}
	if err != nil {
		// The caller leaving says nothing about the host.
		if ctx.Err() != nil {
			return err
		}
		// Older hosts do not answer this subject. Retry capability periodically
		// so a host upgrade does not require restarting the operator.
		s.unsupported[host] = time.Now().Add(time.Minute)
		return err
	}
	s.supported[host] = time.Now()
	for _, status := range response.Workloads {
		s.entries[host+"/"+status.WorkloadId] = statusObservation{status: status, at: at}
	}
	return nil
}

func validateStatusBatch(ids []string, statuses []*runtimev2.WorkloadStatus) error {
	if len(ids) != len(statuses) {
		return fmt.Errorf("incomplete host status batch")
	}
	for i, id := range ids {
		if statuses[i] == nil || statuses[i].WorkloadId != id {
			return fmt.Errorf("unexpected workload in host status batch")
		}
	}
	return nil
}
