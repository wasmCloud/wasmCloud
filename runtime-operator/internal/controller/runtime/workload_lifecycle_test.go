package runtime

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"reflect"
	"strings"
	"testing"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// memoryRefusal is how a host past its memory limit turns a start down.
const memoryRefusal = "process memory limit reached"

// deleteObject removes obj from the fixture's client.
func deleteObject(t *testing.T, r *WorkloadReconciler, obj client.Object) {
	t.Helper()
	if err := r.Delete(t.Context(), obj); err != nil {
		t.Fatal(err)
	}
}

func TestWorkloadConfigValidation(t *testing.T) {
	componentContext := `materializing local resources config for component "` + fixtureComponent + `":`
	serviceContext := "materializing local resources config: "
	for _, tc := range []struct {
		name  string
		setup func(*testing.T, *WorkloadReconciler, *runtimev1alpha1.Workload)
		// wantContext prefixes the NotFound error expected; "" expects none.
		wantContext string
	}{
		{name: "valid"},
		{name: "optional resources", setup: func(_ *testing.T, _ *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			w.Spec.Service.LocalResources = nil
			w.Spec.Components[0].LocalResources = &runtimev1alpha1.LocalResources{}
		}},
		{name: "no service or components", setup: func(_ *testing.T, _ *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			w.Spec.Service = nil
			w.Spec.Components = nil
		}},
		{name: "service environment unset", setup: func(_ *testing.T, _ *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			w.Spec.Service.LocalResources = &runtimev1alpha1.LocalResources{}
		}},
		{name: "assigned host skips validation", setup: func(t *testing.T, r *WorkloadReconciler, w *runtimev1alpha1.Workload) {
			w.Status.HostID = testHeartbeatHostID
			r.Client = interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{
				Get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
					t.Fatal("assigned workload re-read configuration")
					return nil
				},
			})
		}},
		{name: "component config", wantContext: componentContext,
			setup: func(t *testing.T, r *WorkloadReconciler, _ *runtimev1alpha1.Workload) {
				deleteObject(t, r, layerConfigMap(componentLayer))
			}},
		{name: "component secret", wantContext: componentContext,
			setup: func(t *testing.T, r *WorkloadReconciler, _ *runtimev1alpha1.Workload) {
				deleteObject(t, r, layerSecret(componentLayer))
			}},
		{name: "service config", wantContext: serviceContext,
			setup: func(t *testing.T, r *WorkloadReconciler, _ *runtimev1alpha1.Workload) {
				deleteObject(t, r, layerConfigMap(serviceLayer))
			}},
		{name: "service secret", wantContext: serviceContext,
			setup: func(t *testing.T, r *WorkloadReconciler, _ *runtimev1alpha1.Workload) {
				deleteObject(t, r, layerSecret(serviceLayer))
			}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, _ := unplacedFixture(t)
			if tc.setup != nil {
				tc.setup(t, r, workload)
			}
			before := workload.DeepCopy()
			err := r.reconcileConfig(t.Context(), workload)
			if tc.wantContext == "" {
				if err != nil {
					t.Fatal(err)
				}
			} else if !apierrors.IsNotFound(err) || !strings.HasPrefix(err.Error(), tc.wantContext) {
				t.Fatalf("config error = %v, want NotFound starting %q", err, tc.wantContext)
			}
			if !reflect.DeepEqual(before, workload) {
				t.Fatal("config validation mutated the workload")
			}
		})
	}
}

func TestPlacementWaitsForHostSelection(t *testing.T) {
	r, workload, bus := unplacedFixture(t)
	before := workload.DeepCopy()
	if err := r.reconcilePlacement(t.Context(), workload); !condition.IsStatusUnknown(err) {
		t.Fatalf("unselected placement = %v", err)
	}
	if len(bus.requests) != 0 || !reflect.DeepEqual(before, workload) {
		t.Fatal("unselected workload was started or changed")
	}
}

// A start whose answer never arrives, arrives unreadable, or names no workload
// id records nothing, and the retry sends the same request under the same id.
func TestPlacementFailedStartRetriesSameRequest(t *testing.T) {
	errTransport := errors.New("start transport unavailable")
	runningWithoutID := startReply(t, "", runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING, "")
	for _, tc := range []struct {
		name  string
		fail  func(*mockBus)
		check func(error) bool
	}{
		{
			name:  "transport",
			fail:  func(bus *mockBus) { bus.err = errTransport },
			check: func(err error) bool { return errors.Is(err, errTransport) },
		},
		{
			name:  "malformed reply",
			fail:  func(bus *mockBus) { bus.reply = &wasmbus.Message{Data: []byte("not JSON")} },
			check: func(err error) bool { return err != nil },
		},
		{
			name:  "empty reply",
			fail:  func(bus *mockBus) { bus.reply = &wasmbus.Message{Data: []byte("{}")} },
			check: func(err error) bool { return err != nil },
		},
		{
			name: "running without an id",
			fail: func(bus *mockBus) { bus.reply = runningWithoutID },
			check: func(err error) bool {
				return err != nil && strings.Contains(err.Error(), "without a workload id")
			},
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placementFixture(t)
			accepted := bus.reply
			tc.fail(bus)
			before := workload.DeepCopy()
			if err := r.reconcilePlacement(t.Context(), workload); !tc.check(err) {
				t.Fatalf("start error = %v", err)
			}
			if !reflect.DeepEqual(before, workload) || len(bus.requests) != 1 {
				t.Fatalf("failed start changed retry state: %+v", workload.Status)
			}
			bus.err, bus.reply = nil, accepted
			if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatal(err)
			}
			if workload.Status.HostID != testHeartbeatHostID || workload.Status.WorkloadID != fixtureRuntimeID || len(bus.requests) != 2 {
				t.Fatalf("retry status = %+v, calls = %d", workload.Status, len(bus.requests))
			}
			var first, second runtimev2.WorkloadStartRequest
			if err := protojson.Unmarshal(bus.requests[0].Data, &first); err != nil {
				t.Fatal(err)
			}
			if err := protojson.Unmarshal(bus.requests[1].Data, &second); err != nil {
				t.Fatal(err)
			}
			if first.WorkloadId != testWorkloadUID || !proto.Equal(&first, &second) {
				t.Fatal("retry changed the start identity or payload")
			}
			if err := r.reconcilePlacement(t.Context(), workload); err != nil {
				t.Fatal(err)
			}
			if len(bus.requests) != 2 {
				t.Fatal("recorded placement started another workload")
			}
		})
	}
}

// The host answers a start it could not carry out with an error state. That
// is no placement, and its reason reaches the condition, unless the host holds
// the id afterwards: an earlier start of the same workload, or a failure the
// host keeps for Sync to report. An earlier start still under way is waited
// on with the host's reason. Each start turned down holds back the next one
// once the condition records it.
func TestPlacementRefusedStart(t *testing.T) {
	errStatus := errors.New("status unavailable")
	heldAs := func(state runtimev2.WorkloadState, message string) func() (*runtimev2.WorkloadStatusResponse, error) {
		return func() (*runtimev2.WorkloadStatusResponse, error) {
			return &runtimev2.WorkloadStatusResponse{WorkloadStatus: &runtimev2.WorkloadStatus{
				WorkloadId: testWorkloadUID, WorkloadState: state, Message: message,
			}}, nil
		}
	}
	for _, tc := range []struct {
		name, reason string
		// noID leaves the workload id out of the refusal.
		noID bool
		// held is the host's answer to the status check that follows a
		// refusal carrying an id.
		held func() (*runtimev2.WorkloadStatusResponse, error)
		// placed is whether the start counts as the placement, and waiting
		// whether placement waits on the earlier start. wantError is what a
		// refusal wraps, and wantMessage what its error says besides the
		// host's reason.
		placed, waiting bool
		wantError       error
		wantMessage     string
	}{
		{
			name:   "start failed",
			reason: "failed to pull image for component: x509: certificate signed by unknown authority",
			held:   heldAs(runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND, ""),
		},
		{
			name:   "status check fails",
			reason: memoryRefusal,
			held: func() (*runtimev2.WorkloadStatusResponse, error) {
				return nil, errStatus
			},
			wantError: errStatus,
		},
		{
			name: "refusal without a reason",
			held: heldAs(runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND, ""),
		},
		{name: "refusal without an id", reason: "workload id is required", noID: true},
		{
			name:   "replayed start",
			reason: "Workload ID [" + testWorkloadUID + "] already exists",
			held:   heldAs(runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING, ""),
			placed: true,
		},
		{
			name:   "failure the host keeps",
			reason: "component trapped during start",
			held:   heldAs(runtimev2.WorkloadState_WORKLOAD_STATE_ERROR, "component trapped during start"),
			placed: true,
		},
		{
			name:    "earlier start still starting",
			reason:  "Workload ID [" + testWorkloadUID + "] already exists",
			held:    heldAs(runtimev2.WorkloadState_WORKLOAD_STATE_STARTING, ""),
			waiting: true, wantMessage: "WORKLOAD_STATE_STARTING",
		},
		{
			name:    "earlier start failing",
			reason:  "Workload ID [" + testWorkloadUID + "] already exists",
			held:    heldAs(runtimev2.WorkloadState_WORKLOAD_STATE_STOPPING, ""),
			waiting: true, wantMessage: "WORKLOAD_STATE_STOPPING",
		},
		{
			name:   "status request unread",
			reason: memoryRefusal,
			held: func() (*runtimev2.WorkloadStatusResponse, error) {
				return &runtimev2.WorkloadStatusResponse{WorkloadStatus: &runtimev2.WorkloadStatus{
					WorkloadState: runtimev2.WorkloadState_WORKLOAD_STATE_ERROR, Message: "invalid request: bad payload",
				}}, nil
			},
			wantMessage: "invalid request: bad payload",
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placementFixture(t)
			refusedID := testWorkloadUID
			if tc.noID {
				refusedID = ""
			}
			bus.respond = func(message *wasmbus.Message) (*wasmbus.Message, error) {
				if message.Subject == startSubject {
					return refusedReply(t, refusedID, tc.reason), nil
				}
				var request runtimev2.WorkloadStatusRequest
				if err := protojson.Unmarshal(message.Data, &request); err != nil || message.Subject != statusSubject ||
					request.WorkloadId != testWorkloadUID || tc.held == nil {
					t.Fatalf("checked %q for %q (%v), want the refused id", message.Subject, request.WorkloadId, err)
				}
				resp, err := tc.held()
				if err != nil {
					return nil, err
				}
				return rpcReply(t, resp), nil
			}
			wantRequests := 2
			if tc.noID {
				wantRequests = 1
			}
			before := workload.DeepCopy()
			err := r.reconcilePlacement(t.Context(), workload)
			if len(bus.requests) != wantRequests {
				t.Fatalf("sent %d requests, want %d", len(bus.requests), wantRequests)
			}
			// The status check has its own deadline, not what the start left.
			if wantRequests == 2 {
				if left := time.Until(bus.deadlines[1]); left <= 0 || left > workloadStatusTimeout ||
					!bus.deadlines[1].Before(bus.deadlines[0]) {
					t.Fatalf("status check deadline %v away, start's %v", left, time.Until(bus.deadlines[0]))
				}
			}
			if tc.placed {
				if !errors.Is(err, condition.ErrSkipReconciliation()) || workload.Status.WorkloadID != testWorkloadUID {
					t.Fatalf("held start = %v, recorded %q", err, workload.Status.WorkloadID)
				}
				if _, waits := r.startWaits.Load(workload.UID); waits {
					t.Fatal("a placement holds back the next start")
				}
				return
			}
			want := cmp.Or(tc.reason, "WORKLOAD_STATE_ERROR")
			if err == nil || !strings.Contains(err.Error(), want) || !strings.Contains(err.Error(), tc.wantMessage) ||
				condition.IsStatusUnknown(err) != tc.waiting || (tc.wantError != nil && !errors.Is(err, tc.wantError)) {
				t.Fatalf("refused start error = %v, want the host's reason %q", err, want)
			}
			if !reflect.DeepEqual(before, workload) {
				t.Fatalf("refused start was recorded: %+v", workload.Status)
			}
			recordPlacement(workload, err)
			if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, condition.ErrNoop()) ||
				len(bus.requests) != wantRequests {
				t.Fatalf("start after a refusal = %v with %d requests", err, len(bus.requests))
			}
		})
	}
}

// A start answered as running or completed is placed. Any other answer but an
// error state is no placement, its answer reaches the condition, and it holds
// back nothing.
func TestPlacementOtherStartAnswers(t *testing.T) {
	for _, tc := range []struct {
		name    string
		state   runtimev2.WorkloadState
		message string
		placed  bool
	}{
		{name: "completed", state: runtimev2.WorkloadState_WORKLOAD_STATE_COMPLETED, placed: true},
		{name: "stopped while starting", state: runtimev2.WorkloadState_WORKLOAD_STATE_STOPPING,
			message: "Workload was stopped while starting"},
		{name: "starting", state: runtimev2.WorkloadState_WORKLOAD_STATE_STARTING},
		{name: "not found", state: runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND},
		{name: "unspecified", state: runtimev2.WorkloadState_WORKLOAD_STATE_UNSPECIFIED},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placementFixture(t)
			bus.reply = startReply(t, testWorkloadUID, tc.state, tc.message)
			before := workload.DeepCopy()
			err := r.reconcilePlacement(t.Context(), workload)
			if len(bus.requests) != 1 {
				t.Fatalf("sent %d requests, want the start alone", len(bus.requests))
			}
			if tc.placed {
				if !errors.Is(err, condition.ErrSkipReconciliation()) || workload.Status.WorkloadID != testWorkloadUID {
					t.Fatalf("start = %v, recorded %q", err, workload.Status.WorkloadID)
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), tc.state.String()) || !strings.Contains(err.Error(), tc.message) ||
				!reflect.DeepEqual(before, workload) {
				t.Fatalf("start answered %s = %v, recorded %+v", tc.state, err, workload.Status)
			}
			recordPlacement(workload, err)
			if err := r.reconcilePlacement(t.Context(), workload); err == nil || len(bus.requests) != 2 {
				t.Fatalf("retry = %v with %d requests, want a second start", err, len(bus.requests))
			}
		})
	}
}

// A start turned down holds back only the next start to the same host for the
// same spec, only once Placement records the refusal, and only for a reconcile
// interval.
func TestPlacementStartWaitScope(t *testing.T) {
	r, workload, bus := placementFixture(t)
	starts := 0
	bus.respond = func(message *wasmbus.Message) (*wasmbus.Message, error) {
		if !strings.HasSuffix(message.Subject, ".workload.start") {
			t.Fatalf("unexpected RPC %s", message.Subject)
		}
		starts++
		if starts == 5 {
			return startedReply(t), nil
		}
		return refusedReply(t, "", memoryRefusal), nil
	}
	refuse := func(wantStarts int) {
		t.Helper()
		err := r.reconcilePlacement(t.Context(), workload)
		if starts != wantStarts || err == nil || errors.Is(err, condition.ErrNoop()) {
			t.Fatalf("start %d = %v after %d starts", wantStarts, err, starts)
		}
		recordPlacement(workload, err)
	}
	// Unrecorded, as when the status write failed, the refusal holds nothing.
	if err := r.reconcilePlacement(t.Context(), workload); err == nil || starts != 1 {
		t.Fatalf("refused start = %v after %d starts", err, starts)
	}
	refuse(2)
	wait, _ := r.startWaits.Load(workload.UID)
	if left := time.Until(wait.(startWait).until); left > workloadReconcileInterval || left < workloadReconcileInterval-5*time.Second {
		t.Fatalf("next start held back %v, want a reconcile interval", left)
	}
	if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, condition.ErrNoop()) || starts != 2 {
		t.Fatalf("held-back start = %v after %d starts", err, starts)
	}
	workload.Generation++
	refuse(3)
	const otherHost = "pinned-elsewhere"
	workload.Spec.HostID, workload.Status.HostID = otherHost, otherHost
	refuse(4)
	expireStartWaits(r)
	if err := r.reconcilePlacement(t.Context(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) ||
		starts != 5 || workload.Status.WorkloadID != fixtureRuntimeID {
		t.Fatalf("start after the wait = %v, recorded %q", err, workload.Status.WorkloadID)
	}
	if _, waits := r.startWaits.Load(workload.UID); waits {
		t.Fatal("placement kept the wait")
	}
}

// Deleting a Workload that never got a host forgets the start held back for it.
func TestFinalizeForgetsStartWait(t *testing.T) {
	r, workload, bus := unplacedFixture(t)
	r.startWaits.Store(workload.UID, startWait{until: time.Now().Add(workloadReconcileInterval)})
	if err := r.finalize(t.Context(), workload); err != nil {
		t.Fatal(err)
	}
	if _, waits := r.startWaits.Load(workload.UID); waits || len(bus.requests) != 0 {
		t.Fatalf("finalize kept the wait or sent %d requests", len(bus.requests))
	}
}

func TestWorkloadStatusResponses(t *testing.T) {
	for _, tc := range []struct {
		name               string
		state              runtimev2.WorkloadState
		message, wantError string
	}{
		{name: "running", state: runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING},
		{name: "completed", state: runtimev2.WorkloadState_WORKLOAD_STATE_COMPLETED},
		{name: "starting", state: runtimev2.WorkloadState_WORKLOAD_STATE_STARTING,
			wantError: "workload is not operational: WORKLOAD_STATE_STARTING"},
		{name: "stopping", state: runtimev2.WorkloadState_WORKLOAD_STATE_STOPPING,
			wantError: "workload is not operational: WORKLOAD_STATE_STOPPING"},
		{name: "not found", state: runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND,
			wantError: "workload is not operational: WORKLOAD_STATE_NOT_FOUND"},
		{name: "unspecified", state: runtimev2.WorkloadState_WORKLOAD_STATE_UNSPECIFIED,
			wantError: "workload is not operational: WORKLOAD_STATE_UNSPECIFIED"},
		{name: "error without reason", state: runtimev2.WorkloadState_WORKLOAD_STATE_ERROR,
			wantError: "workload is not operational: WORKLOAD_STATE_ERROR"},
		{name: "error with reason", state: runtimev2.WorkloadState_WORKLOAD_STATE_ERROR, message: "minimum memory exceeds host limit",
			wantError: "workload is not operational: WORKLOAD_STATE_ERROR: minimum memory exceeds host limit"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placedFixture(t)
			bus.reply = rpcReply(t, &runtimev2.WorkloadStatusResponse{WorkloadStatus: &runtimev2.WorkloadStatus{
				WorkloadId: fixtureRuntimeID, WorkloadState: tc.state, Message: tc.message,
			}})
			before := workload.DeepCopy()
			err := r.reconcileSync(t.Context(), workload)
			if tc.wantError == "" {
				if err != nil {
					t.Fatal(err)
				}
			} else if err == nil || err.Error() != tc.wantError {
				t.Fatalf("sync error = %v, want %q", err, tc.wantError)
			}
			if !reflect.DeepEqual(before, workload) {
				t.Fatal("status polling mutated the workload")
			}
			if len(bus.requests) != 1 || bus.gotSubject != statusSubject {
				t.Fatalf("sent %d requests, last to %q; want one status poll", len(bus.requests), bus.gotSubject)
			}
			var request runtimev2.WorkloadStatusRequest
			if err := protojson.Unmarshal(bus.gotData, &request); err != nil {
				t.Fatal(err)
			}
			if request.WorkloadId != fixtureRuntimeID {
				t.Fatalf("polled workload %q instead of recorded id", request.WorkloadId)
			}
		})
	}
}

func TestWorkloadSyncFailuresPreservePlacement(t *testing.T) {
	errUnavailable := errors.New("status dependency unavailable")
	isUnavailable := func(err error) bool { return errors.Is(err, errUnavailable) }
	for _, tc := range []struct {
		name  string
		setup func(*WorkloadReconciler, *runtimev1alpha1.Workload, *mockBus)
		check func(error) bool
		polls int
	}{
		{
			name: "not placed",
			setup: func(_ *WorkloadReconciler, w *runtimev1alpha1.Workload, _ *mockBus) {
				w.Status.SetConditions(condition.UnknownCondition(runtimev1alpha1.WorkloadConditionPlacement, "Wait", "not placed"))
			},
			check: condition.IsStatusUnknown,
		},
		{
			name: "host lookup",
			setup: func(r *WorkloadReconciler, _ *runtimev1alpha1.Workload, _ *mockBus) {
				r.Client = interceptor.NewClient(r.Client.(client.WithWatch), interceptor.Funcs{
					List: func(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error {
						return errUnavailable
					},
				})
			},
			check: isUnavailable,
		},
		{
			name:  "transport",
			setup: func(_ *WorkloadReconciler, _ *runtimev1alpha1.Workload, bus *mockBus) { bus.err = errUnavailable },
			check: isUnavailable,
			polls: 1,
		},
		{
			name: "malformed reply",
			setup: func(_ *WorkloadReconciler, _ *runtimev1alpha1.Workload, bus *mockBus) {
				bus.reply = &wasmbus.Message{Data: []byte("not JSON")}
			},
			check: func(err error) bool { return err != nil },
			polls: 1,
		},
		{
			name: "empty reply",
			setup: func(_ *WorkloadReconciler, _ *runtimev1alpha1.Workload, bus *mockBus) {
				bus.reply = &wasmbus.Message{Data: []byte("{}")}
			},
			check: func(err error) bool {
				return err != nil && err.Error() == "workload is not operational: WORKLOAD_STATE_UNSPECIFIED"
			},
			polls: 1,
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			r, workload, bus := placedFixture(t)
			tc.setup(r, workload, bus)
			before := workload.DeepCopy()
			if err := r.reconcileSync(t.Context(), workload); !tc.check(err) {
				t.Fatalf("sync error = %v", err)
			}
			if len(bus.requests) != tc.polls || !reflect.DeepEqual(before, workload) {
				t.Fatalf("sync failure changed placement or sent %d polls: %+v", len(bus.requests), workload.Status)
			}
		})
	}
}

func TestWorkloadReadinessRequiresPlacementAndSync(t *testing.T) {
	statuses := []corev1.ConditionStatus{condition.ConditionUnknown, condition.ConditionFalse, condition.ConditionTrue}
	for _, placement := range statuses {
		for _, sync := range statuses {
			t.Run(string(placement)+"/"+string(sync), func(t *testing.T) {
				workload := &runtimev1alpha1.Workload{}
				workload.Status.SetConditions(
					condition.Condition{Type: runtimev1alpha1.WorkloadConditionPlacement, Status: placement},
					condition.Condition{Type: runtimev1alpha1.WorkloadConditionSync, Status: sync},
				)
				wantReady := placement == condition.ConditionTrue && sync == condition.ConditionTrue
				if err := (&WorkloadReconciler{}).reconcileReady(t.Context(), workload); (err == nil) != wantReady {
					t.Fatalf("ready = %v, want %v", err, wantReady)
				}
			})
		}
	}
}

// recoveringHost fails the first start with errStart, refuses the second with
// refusal and accepts the rest, counting them in starts. A status check finds
// the refused id absent and the accepted one running.
func recoveringHost(t *testing.T, starts *int, errStart error, refusal string) func(*wasmbus.Message) (*wasmbus.Message, error) {
	return func(message *wasmbus.Message) (*wasmbus.Message, error) {
		switch message.Subject {
		case startSubject:
			(*starts)++
			switch *starts {
			case 1:
				return nil, errStart
			case 2:
				return refusedReply(t, testWorkloadUID, refusal), nil
			default:
				return startedReply(t), nil
			}
		case statusSubject:
			var statusRequest runtimev2.WorkloadStatusRequest
			if err := protojson.Unmarshal(message.Data, &statusRequest); err != nil {
				t.Fatal(err)
			}
			state := runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING
			switch statusRequest.WorkloadId {
			case testWorkloadUID:
				state = runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND
			case fixtureRuntimeID:
			default:
				t.Fatalf("polled id = %q", statusRequest.WorkloadId)
			}
			return statusReply(t, statusRequest.WorkloadId, state, ""), nil
		default:
			t.Fatalf("unexpected RPC %s", message.Subject)
			return nil, nil
		}
	}
}

// Through Reconcile and the status it persists: a missing ConfigMap holds the
// Workload before host selection, and once it exists a start that fails and
// one the host refuses are each retried until the Workload is ready, with no
// further start after that.
func TestWorkloadReconcileRecoversFromConfigAndStartFailures(t *testing.T) {
	r, workload, bus := unplacedFixture(t)
	workload.Spec.HostID = testHeartbeatHostID
	var config corev1.ConfigMap
	if err := r.Get(t.Context(), client.ObjectKeyFromObject(layerConfigMap(componentLayer)), &config); err != nil {
		t.Fatal(err)
	}
	deleteObject(t, r, &config)
	if err := r.Create(t.Context(), workload); err != nil {
		t.Fatal(err)
	}
	r.reconciler = r.newReconciler()
	request := ctrl.Request{NamespacedName: client.ObjectKeyFromObject(workload)}
	reconcile := func() *runtimev1alpha1.Workload {
		t.Helper()
		if _, err := r.Reconcile(t.Context(), request); err != nil {
			t.Fatal(err)
		}
		var got runtimev1alpha1.Workload
		if err := r.Get(t.Context(), request.NamespacedName, &got); err != nil {
			t.Fatal(err)
		}
		return &got
	}
	for range 6 {
		got := reconcile()
		if got.Status.GetCondition(runtimev1alpha1.WorkloadConditionConfig).Status != condition.ConditionFalse ||
			got.Status.HostID != "" || len(bus.requests) != 0 {
			t.Fatalf("invalid config allowed placement: %+v", got.Status)
		}
	}
	config.ResourceVersion = ""
	if err := r.Create(t.Context(), &config); err != nil {
		t.Fatal(err)
	}
	startCalls := 0
	errStart := errors.New("transient start failure")
	bus.respond = recoveringHost(t, &startCalls, errStart, memoryRefusal)
	// failures is each distinct Placement message while it is False.
	var failures []string
	ready := false
	for range 16 {
		got := reconcile()
		expireStartWaits(r)
		if placement := got.Status.GetCondition(runtimev1alpha1.WorkloadConditionPlacement); placement.Status == condition.ConditionFalse {
			if got.Status.WorkloadID != "" ||
				got.Status.GetCondition(runtimev1alpha1.WorkloadConditionSync).Status == condition.ConditionTrue {
				t.Fatalf("failed start status = %+v", got.Status)
			}
			if len(failures) == 0 || failures[len(failures)-1] != placement.Message {
				failures = append(failures, placement.Message)
			}
		}
		if got.Status.IsAvailable() {
			if got.Status.WorkloadID != fixtureRuntimeID || got.Status.HostID != testHeartbeatHostID || got.Status.Environment != testTenant {
				t.Fatalf("ready placement = %+v", got.Status)
			}
			ready = true
			break
		}
	}
	if !ready || startCalls != 3 || len(failures) != 2 ||
		!strings.Contains(failures[0], errStart.Error()) || !strings.Contains(failures[1], memoryRefusal) {
		t.Fatalf("failures=%q ready=%v start calls=%d", failures, ready, startCalls)
	}
	reconcile()
	if startCalls != 3 {
		t.Fatal("ready workload was started again")
	}
}

// Through Reconcile: a host that turns each start down for a different reason,
// as a memory reading gives, is sent one start a reconcile interval rather than
// one a pass. Sync is written meanwhile and keeps its transition time, so a
// ReplicaSet can time how long the replica has gone unplaced.
func TestWorkloadReconcileHoldsBackRefusedStarts(t *testing.T) {
	r, workload, bus := unplacedFixture(t)
	workload.Spec.HostID = testHeartbeatHostID
	if err := r.Create(t.Context(), workload); err != nil {
		t.Fatal(err)
	}
	r.reconciler = r.newReconciler()
	starts := 0
	bus.respond = func(message *wasmbus.Message) (*wasmbus.Message, error) {
		if message.Subject == statusSubject {
			return statusReply(t, testWorkloadUID, runtimev2.WorkloadState_WORKLOAD_STATE_NOT_FOUND, ""), nil
		}
		starts++
		return refusedReply(t, testWorkloadUID, fmt.Sprintf(memoryRefusal+": %d MiB in use", 1000+starts)), nil
	}
	request := ctrl.Request{NamespacedName: client.ObjectKeyFromObject(workload)}
	var syncedAt metav1.Time
	for round := range 2 {
		for range 8 {
			if _, err := r.Reconcile(t.Context(), request); err != nil {
				t.Fatal(err)
			}
		}
		var got runtimev1alpha1.Workload
		if err := r.Get(t.Context(), request.NamespacedName, &got); err != nil {
			t.Fatal(err)
		}
		placement := got.Status.GetCondition(runtimev1alpha1.WorkloadConditionPlacement)
		sync := got.Status.GetCondition(runtimev1alpha1.WorkloadConditionSync)
		if round == 0 {
			syncedAt = sync.LastTransitionTime
		}
		if starts != round+1 || placement.Status != condition.ConditionFalse ||
			!strings.Contains(placement.Message, fmt.Sprintf("%d MiB in use", 1001+round)) ||
			sync.Status == condition.ConditionTrue || syncedAt.IsZero() || !sync.LastTransitionTime.Equal(&syncedAt) {
			t.Fatalf("round %d: %d starts, placement %+v, sync %+v", round, starts, placement, sync)
		}
		expireStartWaits(r)
	}
}
