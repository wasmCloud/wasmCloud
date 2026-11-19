package runtime

import (
	"context"
	"errors"
	"fmt"
	"hash/fnv"
	"slices"
	"sync"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"golang.org/x/sync/errgroup"
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
	// statusBatchChunkConcurrency bounds the requests one fetch sends a host
	// for a bucket larger than one batch.
	statusBatchChunkConcurrency = 4
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

// statusBatchMaxIDs and statusBatchMaxIDLen are the host's per-request limits.
// A request past either is refused whole without a reply.
const (
	statusBatchMaxIDs   = 128
	statusBatchMaxIDLen = 128
)

// errHostSilent reports that a host proven to answer batches did not answer
// one. The individual Status RPC would only wait out the same host again.
var errHostSilent = errors.New("host did not answer a status batch")

// cached returns an observation made since placement that is still within
// its ttl. An observation made by the fetch at or after fetched, as for a
// caller that just waited on that fetch, is returned whatever its state; a
// zero fetched (no fetch, or a failed one) applies the ttl to every entry.
func (s *statusBatches) cached(host, id string, placed, fetched time.Time) *runtimev2.WorkloadStatus {
	s.mu.Lock()
	defer s.mu.Unlock()
	entry, ok := s.entries[host+"/"+id]
	// Placement times are stored to the second. An observation within that
	// second may predate the start, so it is not trusted.
	if !placed.IsZero() && placed.Nanosecond() == 0 {
		placed = placed.Add(time.Second)
	}
	if !ok || entry.at.Before(placed) || time.Since(entry.at) >= workloadReconcileInterval {
		return nil
	}
	refreshed := !fetched.IsZero() && !entry.at.Before(fetched)
	if !refreshed && time.Since(entry.at) >= ttl(entry.status) {
		return nil
	}
	return entry.status
}

// status returns the workload's status from a batch, or nil when the caller
// should ask the host directly. errHostSilent means it should not.
func (s *statusBatches) status(ctx context.Context, r *WorkloadReconciler, w *runtimev1alpha1.Workload) (*runtimev2.WorkloadStatus, error) {
	host, id := w.Status.HostID, w.Status.WorkloadID
	if len(id) > statusBatchMaxIDLen {
		return nil, nil
	}
	placed := w.Status.GetCondition(runtimev1alpha1.WorkloadConditionPlacement).LastTransitionTime.Time
	if cached := s.cached(host, id, placed, time.Time{}); cached != nil {
		return cached, nil
	}
	s.mu.Lock()
	unsupported := time.Now().Before(s.unsupported[host])
	s.mu.Unlock()
	if unsupported {
		return nil, nil
	}
	key := statusBucket(host, id)
	// DoChan lets a cancelled reconcile leave without blocking on another
	// caller. The shared fetch covers the whole bucket and returns when its
	// queries started, so each waiter can tell an entry that fetch refreshed
	// from an older one.
	result := s.requests.DoChan(key, func() (any, error) {
		return s.fetch(ctx, r, host, id, key)
	})
	select {
	case <-ctx.Done():
		return nil, nil
	case res := <-result:
		// Only entries stamped by this fetch pass the fetched check, so a
		// partly failed fetch still serves the chunks that answered.
		fetched, _ := res.Val.(time.Time)
		if cached := s.cached(host, id, placed, fetched); cached != nil {
			return cached, nil
		}
		if errors.Is(res.Err, errHostSilent) {
			return nil, res.Err
		}
		return nil, nil
	}
}

func (s *statusBatches) fetch(ctx context.Context, r *WorkloadReconciler, host, id, key string) (time.Time, error) {
	s.mu.Lock()
	proven := time.Since(s.supported[host]) < statusBatchSupportTTL
	s.mu.Unlock()
	if !proven {
		// A host not yet seen answering is probed with this one ID and a short
		// timeout: older hosts never reply, and a loaded host answers a single
		// lookup quickly, so a slow full batch cannot disable batching for it.
		return s.query(ctx, r, host, []string{id}, statusBatchProbeTimeout, false)
	}
	list := &runtimev1alpha1.WorkloadList{}
	if err := r.List(ctx, list, client.MatchingFields{statusBucketIndex: key}); err != nil {
		return time.Time{}, err
	}
	ids := make([]string, 0, len(list.Items)+1)
	ids = append(ids, id)
	for _, w := range list.Items {
		// The host refuses a whole request over its ID limit, so a Workload
		// with an ID it cannot take asks for its status alone.
		if id := w.Status.WorkloadID; id != "" && len(id) <= statusBatchMaxIDLen {
			ids = append(ids, id)
		}
	}
	slices.Sort(ids)
	ids = slices.Compact(ids)
	// Every Syncing Workload in the bucket waits on this fetch, so it covers
	// the whole bucket, one bounded request per chunk.
	var (
		mu      sync.Mutex
		fetched time.Time
	)
	queries, queryCtx := errgroup.WithContext(ctx)
	queries.SetLimit(statusBatchChunkConcurrency)
	for chunk := range slices.Chunk(ids, statusBatchMaxIDs) {
		queries.Go(func() error {
			at, err := s.query(queryCtx, r, host, chunk, workloadStatusTimeout, true)
			mu.Lock()
			defer mu.Unlock()
			if !at.IsZero() && (fetched.IsZero() || at.Before(fetched)) {
				fetched = at
			}
			return err
		})
	}
	err := queries.Wait()
	return fetched, err
}

// query returns when it started, the time its observations are recorded at.
// On a proven host, a request that gets no answer returns errHostSilent.
func (s *statusBatches) query(ctx context.Context, r *WorkloadReconciler, host string, ids []string, timeout time.Duration, proven bool) (time.Time, error) {
	at := time.Now()
	queryCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	var response runtimev2.WorkloadStatusBatchResponse
	err := RoundTrip(queryCtx, r.Bus, NewWashHostClient(r.Bus, host).subject("workload.status.batch"),
		&runtimev2.WorkloadStatusBatchRequest{WorkloadIds: ids}, &response)
	silent := err != nil && proven && ctx.Err() == nil
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
			return time.Time{}, err
		}
		if silent {
			// The host answered batches before; it is not answering anything
			// now, and the Sync failing says so sooner than a second timeout.
			return time.Time{}, fmt.Errorf("%w: %w", errHostSilent, err)
		}
		// Older hosts do not answer this subject. Retry capability periodically
		// so a host upgrade does not require restarting the operator.
		s.unsupported[host] = time.Now().Add(time.Minute)
		return time.Time{}, err
	}
	s.supported[host] = time.Now()
	for _, status := range response.Workloads {
		s.entries[host+"/"+status.WorkloadId] = statusObservation{status: status, at: at}
	}
	return at, nil
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
