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

const statusBucketIndex = "workload.status.host.bucket"

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

func (s *statusBatches) cached(host, id string, placed time.Time) *runtimev2.WorkloadStatus {
	s.mu.Lock()
	defer s.mu.Unlock()
	entry := s.entries[host+"/"+id]
	if entry.at.Before(placed) || time.Since(entry.at) >= workloadReconcileInterval {
		return nil
	}
	return entry.status
}

func (s *statusBatches) status(ctx context.Context, r *WorkloadReconciler, w *runtimev1alpha1.Workload) *runtimev2.WorkloadStatus {
	host, id := w.Status.HostID, w.Status.WorkloadID
	placed := w.Status.GetCondition(runtimev1alpha1.WorkloadConditionPlacement).LastTransitionTime.Time
	if cached := s.cached(host, id, placed); cached != nil {
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
		if cached := s.cached(host, id, placed); cached != nil {
			return nil, nil
		}
		return nil, s.fetch(ctx, r, host, id, key)
	})
	select {
	case <-ctx.Done():
		return nil
	case <-result:
		return s.cached(host, id, placed)
	}
}

func (s *statusBatches) fetch(ctx context.Context, r *WorkloadReconciler, host, id, key string) error {
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
	ids = ids[start:min(start+128, len(ids))]
	at := time.Now()
	queryCtx, cancel := context.WithTimeout(ctx, workloadStatusTimeout)
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
		s.unsupported = make(map[string]time.Time)
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
		s.swept = time.Now()
	}
	if err != nil {
		// Older hosts do not answer this subject. Retry capability periodically
		// so a host upgrade does not require restarting the operator.
		s.unsupported[host] = time.Now().Add(time.Minute)
		return err
	}
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
