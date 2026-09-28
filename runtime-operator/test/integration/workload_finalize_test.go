package integration

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	"google.golang.org/protobuf/encoding/protojson"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

const testHostID = "host-1"

// fakeHost stands in for a wash host and tracks which workloads it runs, so a
// test can detect a workload left running after its Workload is gone.
// holdStart keeps a start in flight, as a real start is while fetching components.
type fakeHost struct {
	mu       sync.Mutex
	running  map[string]bool
	received map[string]chan struct{}
	gates    map[string]chan struct{}
}

func newFakeHost() *fakeHost {
	return &fakeHost{
		running:  map[string]bool{},
		received: map[string]chan struct{}{},
		gates:    map[string]chan struct{}{},
	}
}

// holdStart lets a test delete the Workload while its start is still in flight.
func (h *fakeHost) holdStart(name string) (received <-chan struct{}, release func()) {
	h.mu.Lock()
	defer h.mu.Unlock()
	recv, gate := make(chan struct{}), make(chan struct{})
	h.received[name], h.gates[name] = recv, gate
	return recv, sync.OnceFunc(func() { close(gate) })
}

func (h *fakeHost) isRunning(id string) bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.running[id]
}

func (h *fakeHost) Request(ctx context.Context, msg *wasmbus.Message) (*wasmbus.Message, error) {
	prefix := "runtime.host." + testHostID + "."
	if !strings.HasPrefix(msg.Subject, prefix) {
		return nil, fmt.Errorf("nats: no responders available for request")
	}

	var reply []byte
	var err error
	switch command := strings.TrimPrefix(msg.Subject, prefix); command {
	case "workload.start":
		var req runtimev2.WorkloadStartRequest
		if err := protojson.Unmarshal(msg.Data, &req); err != nil {
			return nil, err
		}
		h.mu.Lock()
		recv, gate := h.received[req.GetWorkload().GetName()], h.gates[req.GetWorkload().GetName()]
		delete(h.received, req.GetWorkload().GetName())
		delete(h.gates, req.GetWorkload().GetName())
		h.mu.Unlock()
		if gate != nil {
			close(recv)
			select {
			case <-gate:
			case <-ctx.Done():
				return nil, ctx.Err()
			}
		}
		h.mu.Lock()
		h.running[req.GetWorkloadId()] = true
		h.mu.Unlock()
		reply, err = protojson.Marshal(&runtimev2.WorkloadStartResponse{
			WorkloadStatus: &runtimev2.WorkloadStatus{
				WorkloadId:    req.GetWorkloadId(),
				WorkloadState: runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING,
			},
		})
	case "workload.status":
		var req runtimev2.WorkloadStatusRequest
		if err := protojson.Unmarshal(msg.Data, &req); err != nil {
			return nil, err
		}
		state := runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND
		if h.isRunning(req.GetWorkloadId()) {
			state = runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING
		}
		reply, err = protojson.Marshal(&runtimev2.WorkloadStatusResponse{
			WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: req.GetWorkloadId(), WorkloadState: state},
		})
	case "workload.stop":
		var req runtimev2.WorkloadStopRequest
		if err := protojson.Unmarshal(msg.Data, &req); err != nil {
			return nil, err
		}
		h.mu.Lock()
		state := runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND
		if h.running[req.GetWorkloadId()] {
			state = runtimev2.WorkloadState_WORKLOAD_STATE_STOPPING
		}
		delete(h.running, req.GetWorkloadId())
		h.mu.Unlock()
		reply, err = protojson.Marshal(&runtimev2.WorkloadStopResponse{
			WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: req.GetWorkloadId(), WorkloadState: state},
		})
	default:
		return nil, fmt.Errorf("fake host: unexpected command %q", command)
	}
	if err != nil {
		return nil, err
	}
	return &wasmbus.Message{Subject: msg.Reply, Data: reply}, nil
}

func (h *fakeHost) Subscribe(string, int) (wasmbus.Subscription, error)              { return nil, nil }
func (h *fakeHost) QueueSubscribe(string, string, int) (wasmbus.Subscription, error) { return nil, nil }
func (h *fakeHost) Publish(*wasmbus.Message) error                                   { return nil }

func newPinnedWorkload(ctx context.Context, name string) *runtimev1alpha1.Workload {
	GinkgoHelper()
	w := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: "default"},
		Spec: runtimev1alpha1.WorkloadSpec{
			HostID: testHostID,
			Components: []runtimev1alpha1.WorkloadComponent{{
				Name:  "http",
				Image: "example.com/http:1",
			}},
		},
	}
	Expect(k8sClient.Create(ctx, w)).To(Succeed())
	return w
}

var _ = Describe("Workload finalizer", func() {
	It("stops a workload on its host once the Workload is gone", func() {
		ctx := context.Background()
		w := newPinnedWorkload(ctx, "stopped-after-placement")
		key := types.NamespacedName{Namespace: w.Namespace, Name: w.Name}

		Eventually(func(g Gomega) {
			var got runtimev1alpha1.Workload
			g.Expect(k8sClient.Get(ctx, key, &got)).To(Succeed())
			g.Expect(got.Status.WorkloadID).NotTo(BeEmpty())
		}).Should(Succeed())
		Expect(testHost.isRunning(string(w.UID))).To(BeTrue())

		Expect(k8sClient.Delete(ctx, w)).To(Succeed())
		Eventually(func() bool {
			return apierrors.IsNotFound(k8sClient.Get(ctx, key, &runtimev1alpha1.Workload{}))
		}).Should(BeTrue())

		Expect(testHost.isRunning(string(w.UID))).To(BeFalse(),
			"the host still runs the workload after its Workload was deleted")
	})

	// The production path behind the orphaned workloads: a rollout deletes the
	// old ReplicaSet while one of its Workloads is being placed. The start is
	// in flight when the Workload gets its deletionTimestamp, and completes
	// afterwards. The host runs the workload from then on, so deleting the
	// Workload must stop it there.
	//
	// Placement records the start in a status patch, and the reconcile that
	// finalizes the Workload is queued by the deletion, so it runs straight
	// after placement. Whether it sees that status depends on whether the
	// patch's watch event reached the operator's cache first. This holds the
	// event back, so the finalizer always decides on the version without it.
	It("stops a workload whose Workload was deleted while its start was in flight", func() {
		ctx := context.Background()
		const name = "deleted-mid-start"
		key := types.NamespacedName{Namespace: "default", Name: name}
		gone := func() bool {
			return apierrors.IsNotFound(k8sClient.Get(ctx, key, &runtimev1alpha1.Workload{}))
		}

		startReceived, releaseStart := testHost.holdStart(name)
		DeferCleanup(releaseStart)
		releasePlacement := testCacheHold.holdPlacement(name)
		DeferCleanup(releasePlacement)

		w := newPinnedWorkload(ctx, name)
		Eventually(startReceived).Should(BeClosed())

		Expect(k8sClient.Delete(ctx, w)).To(Succeed())
		Eventually(func(g Gomega) {
			var cached runtimev1alpha1.Workload
			g.Expect(testCache.Get(ctx, key, &cached)).To(Succeed())
			g.Expect(cached.DeletionTimestamp).NotTo(BeNil())
		}).Should(Succeed())

		releaseStart()

		// Keep the placement out of the cache until the operator has let the
		// Workload go, or long enough that one which waits for a fresh read
		// before it does has had to.
		for deadline := time.Now().Add(2 * time.Second); !gone() && time.Now().Before(deadline); {
			time.Sleep(10 * time.Millisecond)
		}
		releasePlacement()

		Eventually(gone).Should(BeTrue())
		Expect(testHost.isRunning(string(w.UID))).To(BeFalse(),
			"the Workload is gone while the host still runs it")
	})
})

// cacheHold holds a Workload's placed version back from the operator's cache.
// Its transform runs on every Workload event before the informer queues it
// for the cache, so blocking there leaves the cache on the version before, as
// a slow watch does, while reconcilers keep reading it.
type cacheHold struct {
	mu    sync.Mutex
	gates map[string]chan struct{}
}

func newCacheHold() *cacheHold {
	return &cacheHold{gates: map[string]chan struct{}{}}
}

// holdPlacement keeps any version of the named Workload that is being deleted
// and records a workload id out of the cache until release.
func (h *cacheHold) holdPlacement(name string) (release func()) {
	h.mu.Lock()
	defer h.mu.Unlock()
	gate := make(chan struct{})
	h.gates[name] = gate
	return sync.OnceFunc(func() { close(gate) })
}

func (h *cacheHold) transform(obj any) (any, error) {
	w, ok := obj.(*runtimev1alpha1.Workload)
	if !ok || w.DeletionTimestamp == nil || w.Status.WorkloadID == "" {
		return obj, nil
	}
	h.mu.Lock()
	gate := h.gates[w.Name]
	h.mu.Unlock()
	if gate != nil {
		<-gate
	}
	return obj, nil
}
