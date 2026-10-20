package runtime

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"golang.org/x/time/rate"
	"google.golang.org/protobuf/encoding/protojson"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

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
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: testHeartbeatHostID, Namespace: testNamespace},
		HostID:     testHeartbeatHostID,
	}
	r := &WorkloadReconciler{Client: newWorkloadClient(t, host), Bus: bus, OperatorNamespace: testNamespace}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "limits", Namespace: metav1.NamespaceDefault},
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
		Status: runtimev1alpha1.WorkloadStatus{HostID: testHeartbeatHostID},
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

const testWorkloadUID = "workload-uid"

// A Workload deleted mid-start has a host but no recorded workload id; it must still be stopped.
func TestFinalizeStopsWorkloadWithoutRecordedPlacement(t *testing.T) {
	reply, err := protojson.Marshal(&runtimev2.WorkloadStopResponse{
		WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: testWorkloadUID},
	})
	if err != nil {
		t.Fatalf("marshal stop response: %v", err)
	}

	bus := &mockBus{reply: &wasmbus.Message{Data: reply}}
	r := &WorkloadReconciler{Bus: bus}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "deleted-mid-start", Namespace: "default", UID: testWorkloadUID},
		Status:     runtimev1alpha1.WorkloadStatus{HostID: "host-1"},
	}

	if err := r.finalize(context.Background(), workload); err != nil {
		t.Fatalf("finalize: %v", err)
	}

	if want := "runtime.host.host-1.workload.stop"; bus.gotSubject != want {
		t.Fatalf("stop sent to %q, want %q", bus.gotSubject, want)
	}
	var req runtimev2.WorkloadStopRequest
	if err := protojson.Unmarshal(bus.gotData, &req); err != nil {
		t.Fatalf("unmarshal stop request: %v", err)
	}
	if req.GetWorkloadId() != testWorkloadUID {
		t.Errorf("stop named workload %q, want the Workload's UID %q", req.GetWorkloadId(), testWorkloadUID)
	}
}

// No host was ever selected, so no start was sent and there is nothing to stop.
func TestFinalizeSkipsWorkloadWithoutHost(t *testing.T) {
	bus := &mockBus{err: errors.New("no request expected")}
	r := &WorkloadReconciler{Bus: bus}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "unscheduled", Namespace: "default", UID: testWorkloadUID},
	}

	if err := r.finalize(context.Background(), workload); err != nil {
		t.Fatalf("finalize: %v", err)
	}
	if bus.gotSubject != "" {
		t.Errorf("finalize sent %q for a Workload with no host", bus.gotSubject)
	}
}

// newWorkloadClient builds a fake client wired with the HostID index the
// Workload reconciler uses to look up the Host it is placed on.
func newWorkloadClient(t *testing.T, objs ...client.Object) client.Client {
	t.Helper()
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	return fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.Host{}, hostIDIndex,
			func(obj client.Object) []string {
				host, ok := obj.(*runtimev1alpha1.Host)
				if !ok || host.HostID == "" {
					return nil
				}
				return []string{host.HostID}
			}).
		Build()
}

func placedWorkload() *runtimev1alpha1.Workload {
	w := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "placed"},
	}
	w.Status.HostID = testHeartbeatHostID
	w.Status.WorkloadID = "workload-id"
	w.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionPlacement))
	return w
}

// A stop that fails on a host still heartbeating may have left the workload
// running, so the finalizer must stay and the stop be retried.
func TestFinalizeRetriesFailedStopOnReadyHost(t *testing.T) {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "host", Namespace: testNamespace},
		HostID:     testHeartbeatHostID,
	}
	host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	stopErr := errors.New("nats: timeout")
	r := &WorkloadReconciler{
		Client:            newWorkloadClient(t, host),
		Bus:               &mockBus{err: stopErr},
		OperatorNamespace: testNamespace,
	}

	if err := r.finalize(context.Background(), placedWorkload()); !errors.Is(err, stopErr) {
		t.Fatalf("finalize returned %v, want the stop error so it is retried", err)
	}
}

// A host that stopped heartbeating may never answer, and its Host may never be
// reaped, so waiting on it could block the deletion for good.
func TestFinalizeGivesUpFailedStopOnUnavailableHost(t *testing.T) {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "host", Namespace: testNamespace},
		HostID:     testHeartbeatHostID,
	}
	r := &WorkloadReconciler{
		Client:            newWorkloadClient(t, host),
		Bus:               &mockBus{err: errors.New("nats: timeout")},
		OperatorNamespace: testNamespace,
	}

	if err := r.finalize(context.Background(), placedWorkload()); err != nil {
		t.Fatalf("finalize: %v", err)
	}
}

// A Host being deleted is still Ready, but nothing new may be placed on it.
func TestFindFreeHostSkipsHostBeingDeleted(t *testing.T) {
	deleting := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{
			Name:              "deleting",
			Namespace:         testNamespace,
			DeletionTimestamp: &metav1.Time{Time: time.Now()},
			Finalizers:        []string{hostFinalizerName},
		},
		HostID: "deleting-host",
	}
	deleting.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	live := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "live", Namespace: testNamespace},
		HostID:     "live-host",
	}
	live.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))

	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	r := &WorkloadReconciler{
		Client: fake.NewClientBuilder().
			WithScheme(s).
			WithObjects(deleting, live).
			WithIndex(&runtimev1alpha1.Host{}, workloadSchedulableHostsIndex, schedulableHostIndexValue).
			Build(),
		OperatorNamespace: testNamespace,
		AllowSharedHosts:  true,
	}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "pending"},
	}

	// findFreeHost shuffles its candidates, so one pick could miss the bug.
	for range 20 {
		host, err := r.findFreeHost(context.Background(), workload)
		if err != nil {
			t.Fatalf("findFreeHost: %v", err)
		}
		if host.HostID != live.HostID {
			t.Fatalf("placed on host %q, want %q", host.HostID, live.HostID)
		}
	}
}

// Pinning skips findFreeHost, so it must refuse a Host being deleted itself.
func TestHostSelectionWaitsOnPinnedHostBeingDeleted(t *testing.T) {
	deleting := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{
			Name:              "deleting",
			Namespace:         testNamespace,
			DeletionTimestamp: &metav1.Time{Time: time.Now()},
			Finalizers:        []string{hostFinalizerName},
		},
		HostID: testHeartbeatHostID,
	}
	r := &WorkloadReconciler{
		Client:            newWorkloadClient(t, deleting),
		OperatorNamespace: testNamespace,
	}
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "pinned"},
	}
	workload.Spec.HostID = testHeartbeatHostID
	workload.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionConfig))

	if err := r.reconcileHostSelection(context.Background(), workload); err == nil {
		t.Fatal("host selection accepted a pinned host that is being deleted")
	}
	if workload.Status.HostID != "" {
		t.Errorf("recorded HostID %q for a host that is being deleted", workload.Status.HostID)
	}
}

// Pinning skips findFreeHost, so it must apply the Environment rule itself:
// with sharing disabled, spec.hostId cannot reach another tenant's host.
func TestHostSelectionHoldsPinnedHostToEnvironment(t *testing.T) {
	for _, tc := range []struct {
		name        string
		shared      bool
		environment string
		registered  bool
		wantPlaced  bool
		wantEvent   string
	}{
		{name: "other tenant", environment: "tenant-b", registered: true, wantEvent: "CrossEnvironmentSchedulingDenied"},
		{name: "own tenant", environment: "tenant-a", registered: true, wantPlaced: true},
		{name: "unregistered", environment: "tenant-a", wantEvent: "PinnedHostNotRegistered"},
		{name: "shared hosts", shared: true, environment: "tenant-b", registered: true, wantPlaced: true},
	} {
		var objs []client.Object
		if tc.registered {
			objs = append(objs, &runtimev1alpha1.Host{
				ObjectMeta:  metav1.ObjectMeta{Name: "pinned", Namespace: testNamespace},
				HostID:      testHeartbeatHostID,
				Environment: tc.environment,
			})
		}
		recorder := events.NewFakeRecorder(4)
		r := &WorkloadReconciler{
			Client:            newWorkloadClient(t, objs...),
			Recorder:          recorder,
			OperatorNamespace: testNamespace,
			AllowSharedHosts:  tc.shared,
		}
		workload := &runtimev1alpha1.Workload{
			ObjectMeta: metav1.ObjectMeta{Name: "pinned", Namespace: "tenant-a"},
		}
		workload.Spec.HostID = testHeartbeatHostID
		workload.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionConfig))

		err := r.reconcileHostSelection(context.Background(), workload)
		if placed := workload.Status.HostID == testHeartbeatHostID; placed != tc.wantPlaced {
			t.Errorf("%s: placed = %v (err %v), want %v", tc.name, placed, err, tc.wantPlaced)
		}
		if tc.wantPlaced && workload.Status.Environment != tc.environment {
			t.Errorf("%s: Environment = %q, want %q", tc.name, workload.Status.Environment, tc.environment)
		}
		select {
		case event := <-recorder.Events:
			if tc.wantEvent == "" || !strings.Contains(event, tc.wantEvent) {
				t.Errorf("%s: unexpected event %q", tc.name, event)
			}
		default:
			if tc.wantEvent != "" {
				t.Errorf("%s: no %s event", tc.name, tc.wantEvent)
			}
		}
	}
}

// A workload placed before the Environment rule covered it, pinned or before
// allowSharedHosts was turned off, is stopped there and selected again.
func TestHostSelectionEvictsPlacedWorkloadOutsideEnvironment(t *testing.T) {
	stopped, err := protojson.Marshal(&runtimev2.WorkloadStopResponse{
		WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: testWorkloadUID},
	})
	if err != nil {
		t.Fatalf("marshal stop response: %v", err)
	}
	for _, tc := range []struct {
		name        string
		shared      bool
		environment string
		stopErr     error
		limited     bool
		wantEvicted bool
		wantStop    bool
	}{
		{name: "other tenant", environment: "tenant-b", wantEvicted: true, wantStop: true},
		{name: "stop fails", environment: "tenant-b", stopErr: errors.New("nats: timeout"), wantStop: true},
		{name: "paced", environment: "tenant-b", limited: true},
		{name: "own tenant", environment: "tenant-a"},
		{name: "shared hosts", shared: true, environment: "tenant-b"},
	} {
		host := &runtimev1alpha1.Host{
			ObjectMeta:  metav1.ObjectMeta{Name: "placed", Namespace: testNamespace},
			HostID:      testHeartbeatHostID,
			Environment: tc.environment,
		}
		host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
		bus := &mockBus{reply: &wasmbus.Message{Data: stopped}, err: tc.stopErr}
		recorder := events.NewFakeRecorder(4)
		r := &WorkloadReconciler{
			Client:            newWorkloadClient(t, host),
			Bus:               bus,
			Recorder:          recorder,
			OperatorNamespace: testNamespace,
			AllowSharedHosts:  tc.shared,
		}
		if tc.limited {
			r.EvictionLimiter = rate.NewLimiter(0, 0)
		}
		workload := &runtimev1alpha1.Workload{
			ObjectMeta: metav1.ObjectMeta{Name: "placed", Namespace: "tenant-a", UID: testWorkloadUID},
		}
		workload.Status.HostID = testHeartbeatHostID
		workload.Status.WorkloadID = testWorkloadUID
		workload.Status.SetConditions(
			condition.ReadyCondition(runtimev1alpha1.WorkloadConditionPlacement),
			condition.ReadyCondition(runtimev1alpha1.WorkloadConditionSync),
			condition.ReadyCondition(condition.TypeReady),
		)

		err := r.reconcileHostSelection(context.Background(), workload)
		if evicted := workload.Status.HostID == ""; evicted != tc.wantEvicted {
			t.Errorf("%s: evicted = %v (err %v), want %v", tc.name, evicted, err, tc.wantEvicted)
		}
		if tc.wantEvicted && workload.Status.WorkloadID != "" {
			t.Errorf("%s: kept WorkloadID %q after eviction", tc.name, workload.Status.WorkloadID)
		}
		// Nothing after HostSelection may still say the stopped workload runs.
		if tc.wantEvicted && !workload.Status.AnyUnknown(condition.TypeReady) {
			t.Errorf("%s: Ready = %s after eviction", tc.name, workload.Status.GetCondition(condition.TypeReady).Status)
		}
		if tc.limited && err == nil {
			t.Errorf("%s: a paced eviction reported nothing", tc.name)
		}
		if stopSent := bus.gotSubject != ""; stopSent != tc.wantStop {
			t.Errorf("%s: stop sent = %v, want %v", tc.name, stopSent, tc.wantStop)
		}
		if (err != nil) != (tc.wantStop || tc.limited) {
			t.Errorf("%s: err = %v", tc.name, err)
		}
		// A host that did not answer is not asked again within the interval.
		if tc.stopErr != nil {
			bus.gotSubject = ""
			again := r.reconcileHostSelection(context.Background(), workload)
			if bus.gotSubject != "" || !condition.IsStatusUnknown(again) {
				t.Errorf("%s: retried the stop at once (sent %q, err %v)", tc.name, bus.gotSubject, again)
			}
		}
		// Recorded once the stop goes through, not on each failed attempt.
		if got := drainEvents(recorder); tc.wantEvicted != (len(got) == 1 && strings.Contains(got[0], "CrossEnvironmentSchedulingDenied")) {
			t.Errorf("%s: events = %q", tc.name, got)
		}
	}
}

func deletingHost(hostID string) *runtimev1alpha1.Host {
	return &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{
			Name:              hostID,
			Namespace:         testNamespace,
			DeletionTimestamp: &metav1.Time{Time: time.Now()},
			Finalizers:        []string{hostFinalizerName},
		},
		HostID: hostID,
	}
}

// A host chosen before its deletion began must not get the start: its
// finalizer has listed its Workloads already and would miss this one.
func TestPlacementSkipsHostBeingDeleted(t *testing.T) {
	for _, pinned := range []bool{false, true} {
		bus := &mockBus{err: errors.New("no request expected")}
		r := &WorkloadReconciler{
			Client:            newWorkloadClient(t, deletingHost(testHeartbeatHostID)),
			Bus:               bus,
			OperatorNamespace: testNamespace,
		}
		workload := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{Name: "chosen"}}
		workload.Status.HostID = testHeartbeatHostID
		if pinned {
			workload.Spec.HostID = testHeartbeatHostID
		}

		if err := r.reconcilePlacement(context.Background(), workload); err == nil {
			t.Fatalf("pinned=%v: placement succeeded on a host being deleted", pinned)
		}
		if bus.gotSubject != "" {
			t.Errorf("pinned=%v: sent %q to a host being deleted", pinned, bus.gotSubject)
		}
		// Unpinned goes back to host selection; pinned has nowhere else to go.
		wantHostID := ""
		if pinned {
			wantHostID = testHeartbeatHostID
		}
		if workload.Status.HostID != wantHostID {
			t.Errorf("pinned=%v: HostID = %q, want %q", pinned, workload.Status.HostID, wantHostID)
		}
	}
}

// A chosen host whose Host is already gone is not started on either. A pinned
// one may simply not be registered yet, so its start is still sent.
func TestPlacementSkipsHostThatIsGone(t *testing.T) {
	for _, pinned := range []bool{false, true} {
		bus := &mockBus{err: errors.New("host unreachable")}
		r := &WorkloadReconciler{Client: newWorkloadClient(t), Bus: bus, OperatorNamespace: testNamespace}
		workload := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{Name: "chosen"}}
		workload.Status.HostID = testHeartbeatHostID
		if pinned {
			workload.Spec.HostID = testHeartbeatHostID
		}

		_ = r.reconcilePlacement(context.Background(), workload)
		if sent := bus.gotSubject != ""; sent != pinned {
			t.Errorf("pinned=%v: start sent = %v", pinned, sent)
		}
		if !pinned && workload.Status.HostID != "" {
			t.Errorf("an unpinned workload kept HostID %q for a host that is gone", workload.Status.HostID)
		}
	}
}

// A draining host still answers status, so sync has to read the Host itself.
func TestSyncFailsWhenHostIsGoingAway(t *testing.T) {
	for name, objs := range map[string][]client.Object{
		"deleted":       nil,
		"being deleted": {deletingHost(testHeartbeatHostID)},
	} {
		bus := &mockBus{err: errors.New("no request expected")}
		r := &WorkloadReconciler{
			Client:            newWorkloadClient(t, objs...),
			Bus:               bus,
			OperatorNamespace: testNamespace,
		}

		if err := r.reconcileSync(context.Background(), placedWorkload()); err == nil {
			t.Errorf("%s: sync succeeded for a workload whose host is %s", name, name)
		}
		if bus.gotSubject != "" {
			t.Errorf("%s: asked the host for status anyway (%q)", name, bus.gotSubject)
		}
	}

	live := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "live", Namespace: testNamespace},
		HostID:     testHeartbeatHostID,
	}
	bus := &mockBus{err: errors.New("host unreachable")}
	r := &WorkloadReconciler{Client: newWorkloadClient(t, live), Bus: bus, OperatorNamespace: testNamespace}
	_ = r.reconcileSync(context.Background(), placedWorkload())
	if bus.gotSubject == "" {
		t.Errorf("sync never asked a live host for the workload's status")
	}
}

// Once the Host is gone there is nothing left to stop the workload on.
func TestFinalizeGivesUpFailedStopWhenHostIsGone(t *testing.T) {
	r := &WorkloadReconciler{
		Client:            newWorkloadClient(t),
		Bus:               &mockBus{err: errors.New("nats: no responders available for request")},
		OperatorNamespace: testNamespace,
	}

	if err := r.finalize(context.Background(), placedWorkload()); err != nil {
		t.Fatalf("finalize: %v", err)
	}
}

// A blip stalls a host's evictions for one stop timeout; only failures in a row
// stretch it, up to a reconcile interval, and an answer clears it.
func TestStalledHostsBackOff(t *testing.T) {
	var s stalledHosts
	if s.stalled("h") {
		t.Fatal("a host that never failed is stalled")
	}
	waits := []time.Duration{}
	for range 5 {
		s.failed("h")
		waits = append(waits, time.Until(s.stalls["h"].until).Round(time.Second))
	}
	want := []time.Duration{5 * time.Second, 10 * time.Second, 20 * time.Second, 30 * time.Second, 30 * time.Second}
	for i := range want {
		if waits[i] != want[i] {
			t.Fatalf("waits = %v, want %v", waits, want)
		}
	}
	if !s.stalled("h") {
		t.Fatal("a failing host is not stalled")
	}
	s.answered("h")
	if s.stalled("h") {
		t.Fatal("an answer did not clear the stall")
	}
	// A host whose stall ended long ago, such as one since removed, is
	// forgotten the next time any host fails.
	s.stalls["removed"] = hostStall{failures: 3, until: time.Now().Add(-2 * workloadReconcileInterval)}
	s.failed("other")
	if _, kept := s.stalls["removed"]; kept {
		t.Error("a long-expired stall was kept")
	}
}

// Eviction gives up on a stop only for a host known to have failed, and only
// the first workload there spends a stop timeout: the rest are dropped without
// asking. A host that is merely not reporting may still run its workloads, so
// they are kept until a stop goes through.
func TestEvictionGivesUpOnlyOnFailedHost(t *testing.T) {
	for _, tc := range []struct {
		name        string
		ready       condition.Condition
		wantDropped bool
	}{
		{name: "failed", ready: condition.ErrorCondition(condition.TypeReady, "Reconcile", errors.New("host has not reported recently")), wantDropped: true},
		{name: "not reporting", ready: condition.UnknownCondition(condition.TypeReady, "Reconcile", "host is not reporting")},
	} {
		host := &runtimev1alpha1.Host{
			ObjectMeta:  metav1.ObjectMeta{Name: "quiet", Namespace: testNamespace},
			HostID:      testHeartbeatHostID,
			Environment: "tenant-b",
		}
		host.Status.SetConditions(tc.ready)
		bus := &mockBus{err: errors.New("nats: no responders available for request")}
		r := &WorkloadReconciler{Client: newWorkloadClient(t, host), Bus: bus, OperatorNamespace: testNamespace}
		placed := func(name string) *runtimev1alpha1.Workload {
			w := &runtimev1alpha1.Workload{
				ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: "tenant-a", UID: types.UID(name)},
			}
			w.Status.HostID = testHeartbeatHostID
			return w
		}

		first := placed("first")
		_ = r.reconcileHostSelection(context.Background(), first)
		if dropped := first.Status.HostID == ""; dropped != tc.wantDropped || bus.gotSubject == "" {
			t.Errorf("%s: first dropped = %v, stop sent %q; want dropped = %v after one try",
				tc.name, dropped, bus.gotSubject, tc.wantDropped)
		}

		bus.gotSubject = ""
		second := placed("second")
		_ = r.reconcileHostSelection(context.Background(), second)
		if dropped := second.Status.HostID == ""; dropped != tc.wantDropped || bus.gotSubject != "" {
			t.Errorf("%s: second dropped = %v, stop sent %q; want dropped = %v without asking",
				tc.name, dropped, bus.gotSubject, tc.wantDropped)
		}
	}
}

// The wait for an unregistered pinned host is recorded once, when it begins.
func TestPinnedHostNotRegisteredRecordedOnce(t *testing.T) {
	recorder := events.NewFakeRecorder(4)
	r := &WorkloadReconciler{Client: newWorkloadClient(t), Recorder: recorder, OperatorNamespace: testNamespace}
	workload := &runtimev1alpha1.Workload{ObjectMeta: metav1.ObjectMeta{Name: "pinned", Namespace: "tenant-a"}}
	workload.Spec.HostID = testHeartbeatHostID
	workload.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionConfig))

	for pass := range 4 {
		err := r.reconcileHostSelection(context.Background(), workload)
		// What the conditioned reconciler records from the returned error.
		workload.Status.SetConditions(condition.UnknownCondition(
			runtimev1alpha1.WorkloadConditionHostSelection, "Reconcile", err.Error()))
		// A pass that failed another way in between does not restart the wait.
		if pass == 1 {
			workload.Status.SetConditions(condition.ErrorCondition(
				runtimev1alpha1.WorkloadConditionHostSelection, "Reconcile", errors.New("list hosts: timeout")))
		}
	}
	if got := drainEvents(recorder); len(got) != 1 {
		t.Fatalf("events = %q, want one when the wait began", got)
	}

	// Pinned to another unregistered host, it is a new wait.
	workload.Spec.HostID = "another-host"
	_ = r.reconcileHostSelection(context.Background(), workload)
	if got := drainEvents(recorder); len(got) != 1 || !strings.Contains(got[0], "another-host") {
		t.Errorf("events = %q, want one for the new host", got)
	}
}
