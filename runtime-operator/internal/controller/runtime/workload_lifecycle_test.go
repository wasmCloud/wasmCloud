package runtime

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"

	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

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

// A start whose answer never arrives, or arrives unreadable, records nothing,
// and the retry sends the same request under the same workload id.
func TestPlacementFailedStartRetriesSameRequest(t *testing.T) {
	errTransport := errors.New("start transport unavailable")
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

// Through Reconcile and the status it persists: a missing ConfigMap holds the
// Workload before host selection, and once it exists a failed start is
// retried until the Workload is ready, without a second start after that.
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
	bus.respond = func(message *wasmbus.Message) (*wasmbus.Message, error) {
		switch message.Subject {
		case startSubject:
			startCalls++
			if startCalls == 1 {
				return nil, errStart
			}
			return startedReply(t), nil
		case statusSubject:
			var statusRequest runtimev2.WorkloadStatusRequest
			if err := protojson.Unmarshal(message.Data, &statusRequest); err != nil {
				t.Fatal(err)
			}
			if statusRequest.WorkloadId != fixtureRuntimeID {
				t.Fatalf("polled id = %q", statusRequest.WorkloadId)
			}
			return rpcReply(t, &runtimev2.WorkloadStatusResponse{WorkloadStatus: &runtimev2.WorkloadStatus{
				WorkloadId: fixtureRuntimeID, WorkloadState: runtimev2.WorkloadState_WORKLOAD_STATE_RUNNING,
			}}), nil
		default:
			t.Fatalf("unexpected RPC %s", message.Subject)
			return nil, nil
		}
	}
	sawStartFailure, ready := false, false
	for range 12 {
		got := reconcile()
		if placement := got.Status.GetCondition(runtimev1alpha1.WorkloadConditionPlacement); placement.Status == condition.ConditionFalse {
			sawStartFailure = true
			if !strings.Contains(placement.Message, errStart.Error()) || got.Status.WorkloadID != "" {
				t.Fatalf("start failure status = %+v", got.Status)
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
	if !sawStartFailure || !ready || startCalls != 2 {
		t.Fatalf("failure=%v ready=%v start calls=%d", sawStartFailure, ready, startCalls)
	}
	reconcile()
	if startCalls != 2 {
		t.Fatal("ready workload was started again")
	}
}
