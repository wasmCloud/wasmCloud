package runtime

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"google.golang.org/protobuf/encoding/protojson"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/tools/events"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

var errAPIUnavailable = apierrors.NewServiceUnavailable("apiserver unavailable")

// workloadHostIndexes returns the index functions the Workload reconciler
// registers, so tests index Hosts exactly as the manager would.
func workloadHostIndexes(t *testing.T) map[string]client.IndexerFunc {
	t.Helper()
	m := newRecordingManager(t)
	if err := (&WorkloadReconciler{Client: m.GetClient()}).SetupWithManager(m); err != nil {
		t.Fatalf("set up workload reconciler: %v", err)
	}
	return m.indexes
}

// schedulingClient is a fake client with the Host indexes the Workload
// reconciler registers.
func schedulingClient(t *testing.T, funcs interceptor.Funcs, objs ...client.Object) client.WithWatch {
	t.Helper()
	indexes := workloadHostIndexes(t)
	builder := fake.NewClientBuilder().
		WithScheme(newScaleTestScheme(t)).
		WithObjects(objs...).
		WithStatusSubresource(&runtimev1alpha1.Workload{}).
		WithInterceptorFuncs(funcs)
	for _, index := range []string{hostIDIndex, workloadSchedulableHostsIndex, hostEnvironmentIndex} {
		builder = builder.WithIndex(&runtimev1alpha1.Host{}, index, indexes[index])
	}
	return builder.Build()
}

func readyHost(name, environment string) *runtimev1alpha1.Host {
	host := &runtimev1alpha1.Host{
		ObjectMeta:  metav1.ObjectMeta{Name: name, Namespace: testNamespace},
		HostID:      name + "-id",
		Environment: environment,
	}
	host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	return host
}

func configuredWorkload() *runtimev1alpha1.Workload {
	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "scheduled", Namespace: testTenant, UID: testWorkloadUID},
	}
	workload.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadConditionConfig))
	return workload
}

func failHostList(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error {
	return errAPIUnavailable
}

// An unpinned workload is placed on a schedulable host, and records that
// host's Environment for the ENVIRONMENT column.
func TestHostSelectionPlacesUnpinnedWorkload(t *testing.T) {
	host := readyHost("free", otherTenant)
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{}, host),
		OperatorNamespace: testNamespace,
		AllowSharedHosts:  true,
	}
	workload := configuredWorkload()

	if err := r.reconcileHostSelection(context.Background(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) {
		t.Fatalf("host selection returned %v, want a skip once placed", err)
	}
	if workload.Status.HostID != host.HostID || workload.Status.Environment != otherTenant {
		t.Fatalf("placed on %q in %q, want %q in %q",
			workload.Status.HostID, workload.Status.Environment, host.HostID, otherTenant)
	}
}

// With no host to take it, the workload stays unplaced and the failure is
// reported on its condition.
func TestHostSelectionReportsNoFreeHost(t *testing.T) {
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{}),
		OperatorNamespace: testNamespace,
		AllowSharedHosts:  true,
	}
	workload := configuredWorkload()

	err := r.reconcileHostSelection(context.Background(), workload)
	if err == nil || !strings.Contains(err.Error(), "no suitable host") {
		t.Fatalf("host selection returned %v, want no suitable host", err)
	}
	if workload.Status.HostID != "" {
		t.Errorf("recorded HostID %q with no host available", workload.Status.HostID)
	}
}

// With sharing disabled, only the workload's own tenant's hosts are candidates.
func TestFindFreeHostKeepsToOwnEnvironment(t *testing.T) {
	own := readyHost("own", testTenant)
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{}, readyHost("other", otherTenant), own),
		OperatorNamespace: testNamespace,
	}
	// findFreeHost shuffles its candidates, so one pick could miss the bug.
	for range 20 {
		host, err := r.findFreeHost(context.Background(), configuredWorkload())
		if err != nil {
			t.Fatalf("findFreeHost: %v", err)
		}
		if host.HostID != own.HostID {
			t.Fatalf("placed on %q in %q, want the workload's own tenant", host.HostID, host.Environment)
		}
	}
}

// The schedulable index is read from a cache that can lag a Host going away,
// so a host it returns is checked again before it is chosen.
func TestFindFreeHostRechecksIndexedHosts(t *testing.T) {
	stale := readyHost("stale", "")
	stale.Status.SetConditions(condition.ErrorCondition(condition.TypeReady, "Unreachable", errors.New("no heartbeat")))
	r := &WorkloadReconciler{
		Client: schedulingClient(t, interceptor.Funcs{
			List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
				hosts, ok := list.(*runtimev1alpha1.HostList)
				if !ok {
					return c.List(ctx, list, opts...)
				}
				// Answer as an index that has not caught up would: the
				// Host is listed as schedulable though its own conditions
				// now say otherwise.
				hosts.Items = []runtimev1alpha1.Host{*stale}
				return nil
			},
		}),
		OperatorNamespace: testNamespace,
		AllowSharedHosts:  true,
	}
	if host, err := r.findFreeHost(context.Background(), configuredWorkload()); err == nil {
		t.Fatalf("chose host %q that is no longer schedulable", host.HostID)
	}
}

func TestFindFreeHostFailures(t *testing.T) {
	t.Run("list fails", func(t *testing.T) {
		r := &WorkloadReconciler{
			Client:            schedulingClient(t, interceptor.Funcs{List: failHostList}, readyHost("free", "")),
			OperatorNamespace: testNamespace,
			AllowSharedHosts:  true,
		}
		if _, err := r.findFreeHost(context.Background(), configuredWorkload()); !errors.Is(err, errAPIUnavailable) {
			t.Fatalf("findFreeHost returned %v, want the list error", err)
		}
	})

	t.Run("cross-environment target refused", func(t *testing.T) {
		recorder := events.NewFakeRecorder(4)
		r := &WorkloadReconciler{
			Client:            schedulingClient(t, interceptor.Funcs{}, readyHost("other", otherTenant)),
			Recorder:          recorder,
			OperatorNamespace: testNamespace,
		}
		workload := configuredWorkload()
		workload.Spec.Environment = otherTenant
		if _, err := r.findFreeHost(context.Background(), workload); !errors.Is(err, errCrossEnvironmentDenied) {
			t.Fatalf("findFreeHost returned %v, want a cross-environment denial", err)
		}
		if got := drainEvents(recorder); len(got) != 1 || !strings.Contains(got[0], "CrossEnvironmentSchedulingDenied") {
			t.Errorf("events = %q", got)
		}
	})
}

// A pinned workload is held to the Environment rule like any other: a
// cross-tenant spec.environment is refused before the pinned host is read.
func TestHostSelectionRefusesPinnedCrossEnvironmentTarget(t *testing.T) {
	recorder := events.NewFakeRecorder(4)
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{}, readyHost("other", otherTenant)),
		Recorder:          recorder,
		OperatorNamespace: testNamespace,
	}
	workload := configuredWorkload()
	workload.Spec.HostID = "other-id"
	workload.Spec.Environment = otherTenant

	if err := r.reconcileHostSelection(context.Background(), workload); !errors.Is(err, errCrossEnvironmentDenied) {
		t.Fatalf("host selection returned %v, want a cross-environment denial", err)
	}
	if workload.Status.HostID != "" {
		t.Errorf("recorded HostID %q for a refused target", workload.Status.HostID)
	}
	if got := drainEvents(recorder); len(got) != 1 || !strings.Contains(got[0], "CrossEnvironmentSchedulingDenied") {
		t.Errorf("events = %q", got)
	}
}

// When isolation needs the pinned host's Environment and the Host cannot be
// read, placement waits on the read instead of guessing.
func TestHostSelectionWaitsWhenPinnedHostCannotBeRead(t *testing.T) {
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{List: failHostList}),
		OperatorNamespace: testNamespace,
	}
	workload := configuredWorkload()
	workload.Spec.HostID = testHeartbeatHostID

	if err := r.reconcileHostSelection(context.Background(), workload); !errors.Is(err, errAPIUnavailable) {
		t.Fatalf("host selection returned %v, want the read error", err)
	}
	if workload.Status.HostID != "" {
		t.Errorf("placed on %q without knowing its Environment", workload.Status.HostID)
	}
}

// With no Environment rule to apply, a pinned workload is placed even when its
// Host cannot be read yet; the Environment column is simply left empty.
func TestHostSelectionPlacesPinnedWorkloadWithoutReadingHost(t *testing.T) {
	for name, funcs := range map[string]interceptor.Funcs{
		"not registered": {},
		"read fails":     {List: failHostList},
	} {
		t.Run(name, func(t *testing.T) {
			r := &WorkloadReconciler{
				Client:            schedulingClient(t, funcs),
				OperatorNamespace: testNamespace,
				AllowSharedHosts:  true,
			}
			workload := configuredWorkload()
			workload.Spec.HostID = testHeartbeatHostID

			if err := r.reconcileHostSelection(context.Background(), workload); !errors.Is(err, condition.ErrSkipReconciliation()) {
				t.Fatalf("host selection returned %v, want a skip once placed", err)
			}
			if workload.Status.HostID != testHeartbeatHostID || workload.Status.Environment != "" {
				t.Errorf("placed on %q in %q", workload.Status.HostID, workload.Status.Environment)
			}
		})
	}
}

func placedOutside(environment string) *runtimev1alpha1.Workload {
	workload := configuredWorkload()
	workload.Spec.Environment = environment
	workload.Status.HostID = testHeartbeatHostID
	workload.Status.WorkloadID = testWorkloadUID
	workload.Status.SetConditions(
		condition.ReadyCondition(runtimev1alpha1.WorkloadConditionPlacement),
		condition.ReadyCondition(runtimev1alpha1.WorkloadConditionSync),
		condition.ReadyCondition(condition.TypeReady),
	)
	return workload
}

// A placed workload whose own spec.environment is no longer allowed is
// stopped and unplaced, and the reason names its refused Environment.
func TestEvictionForRefusedSpecEnvironment(t *testing.T) {
	stopped, err := protojson.Marshal(&runtimev2.WorkloadStopResponse{
		WorkloadStatus: &runtimev2.WorkloadStatus{WorkloadId: testWorkloadUID},
	})
	if err != nil {
		t.Fatalf("marshal stop response: %v", err)
	}
	host := readyHost("placed", otherTenant)
	host.HostID = testHeartbeatHostID
	bus := &mockBus{reply: &wasmbus.Message{Data: stopped}}
	recorder := events.NewFakeRecorder(4)
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{}, host),
		Bus:               bus,
		Recorder:          recorder,
		OperatorNamespace: testNamespace,
	}
	workload := placedOutside(otherTenant)

	err = r.reconcileHostSelection(context.Background(), workload)
	// ErrStatusUnknown keeps the cause as text, not as a wrapped error.
	if !condition.IsStatusUnknown(err) || !strings.Contains(err.Error(), errCrossEnvironmentDenied.Error()) {
		t.Fatalf("host selection returned %v, want the denial as an unknown condition", err)
	}
	if bus.gotSubject == "" {
		t.Error("the workload was unplaced without being stopped")
	}
	if workload.Status.HostID != "" || workload.Status.WorkloadID != "" {
		t.Errorf("still placed on %q as %q", workload.Status.HostID, workload.Status.WorkloadID)
	}
	if got := drainEvents(recorder); len(got) != 1 || !strings.Contains(got[0], "CrossEnvironmentSchedulingDenied") {
		t.Errorf("events = %q", got)
	}
}

// Eviction that cannot read the Host it would stop on retries rather than
// unplacing a workload that may still be running there.
func TestEvictionWaitsWhenHostCannotBeRead(t *testing.T) {
	bus := &mockBus{err: errors.New("no request expected")}
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{List: failHostList}),
		Bus:               bus,
		OperatorNamespace: testNamespace,
	}
	workload := placedOutside("")

	if err := r.reconcileHostSelection(context.Background(), workload); !errors.Is(err, errAPIUnavailable) {
		t.Fatalf("host selection returned %v, want the read error", err)
	}
	if workload.Status.HostID != testHeartbeatHostID || bus.gotSubject != "" {
		t.Errorf("acted without reading the host: HostID %q, sent %q", workload.Status.HostID, bus.gotSubject)
	}
}

// A stop that fails is asked again only if the Host still says it may be
// running the workload; a failure to read the Host is returned as is.
func TestStopOnHostLookupFailure(t *testing.T) {
	r := &WorkloadReconciler{
		Client:            schedulingClient(t, interceptor.Funcs{List: failHostList}),
		Bus:               &mockBus{err: errors.New("nats: timeout")},
		OperatorNamespace: testNamespace,
	}
	answered, err := r.stopOnHost(context.Background(), placedWorkload(), time.Second, hostFailed)
	if answered || !errors.Is(err, errAPIUnavailable) {
		t.Fatalf("stopOnHost = %v, %v; want the lookup error", answered, err)
	}
}
