package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/go-logr/logr"
	"google.golang.org/protobuf/encoding/protojson"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/envtest"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

// Shared test fixtures for the values a host reports about itself, factored
// out so repeated literals stay consistent across the suite.
const (
	testOSArch   = "arm64"
	testOSName   = "linux"
	testOSKernel = "6.1.0"
	testVersion  = "1.2.3"
	testCPUUsage = "0.5"
	// testHeartbeatHostID is the host these tests heartbeat as. Distinct from
	// workload_route_controller_test.go's testHostID, which names the host a
	// route is expected to resolve to.
	testHeartbeatHostID = "host-id-1"
)

// requiredHostStatusKeys are the status fields the Host CRD marks as required.
// The heartbeat handler's status write is rejected unless every one of these
// keys is present in the patched object.
var requiredHostStatusKeys = []string{
	"version", "osName", "osArch", "osKernel",
	"systemCPUUsage", "systemMemoryTotal", "systemMemoryFree",
}

func patchStatusKeys(t *testing.T, status *runtimev1alpha1.HostStatus) map[string]json.RawMessage {
	t.Helper()
	raw, err := hostStatusPatch(status)
	if err != nil {
		t.Fatalf("hostStatusPatch: %v", err)
	}
	var p struct {
		Status map[string]json.RawMessage `json:"status"`
	}
	if err := json.Unmarshal(raw, &p); err != nil {
		t.Fatalf("unmarshal patch %s: %v", raw, err)
	}
	return p.Status
}

// TestHostStatusPatch_EmptyStatusInjectsRequiredKeys ensures that lastSeen is always
// populated in the patch, even for an empty status. Other keys required are also tested
// to ensure the patch is valid for the CRD.
func TestHostStatusPatch_EmptyStatusInjectsRequiredKeys(t *testing.T) {
	keys := patchStatusKeys(t, &runtimev1alpha1.HostStatus{})

	if _, ok := keys["lastSeen"]; !ok {
		t.Errorf("lastSeen must always be present in the patch")
	}
	for _, k := range requiredHostStatusKeys {
		if _, ok := keys[k]; !ok {
			t.Errorf("required key %q missing from patch for empty status", k)
		}
	}
	// Conditions and the optional counts must never be in this patch — they are
	// owned by the ConditionedReconciler / reconcileReporting respectively.
	for _, k := range []string{"conditions", "componentCount", "workloadCount"} {
		if _, ok := keys[k]; ok {
			t.Errorf("patch must not include %q", k)
		}
	}
}

// TestHostStatusPatch_ReportedFieldsPreserved ensures already-reported values
// are omitted from the patch so the heartbeat handler never clobbers real data
// written by reconcileReporting — only lastSeen is refreshed.
func TestHostStatusPatch_ReportedFieldsPreserved(t *testing.T) {
	keys := patchStatusKeys(t, &runtimev1alpha1.HostStatus{
		Version:           testVersion,
		OSName:            testOSName,
		OSArch:            testOSArch,
		OSKernel:          testOSKernel,
		SystemCPUUsage:    testCPUUsage,
		SystemMemoryTotal: 16000,
		SystemMemoryFree:  8000,
	})

	if _, ok := keys["lastSeen"]; !ok {
		t.Errorf("lastSeen must always be present in the patch")
	}
	for _, k := range requiredHostStatusKeys {
		if _, ok := keys[k]; ok {
			t.Errorf("reported field %q must be omitted to avoid clobbering its real value", k)
		}
	}
}

// TestHostStatusPatch_SatisfiesCRDRequired exercises the patch against a real
// API server with the Host CRD installed.
func startHostEnvtest(t *testing.T) (client.Client, context.Context) {
	t.Helper()
	if os.Getenv("KUBEBUILDER_ASSETS") == "" {
		t.Skip("KUBEBUILDER_ASSETS not set; run via `make test` for envtest")
	}

	env := &envtest.Environment{
		CRDDirectoryPaths:     []string{filepath.Join("..", "..", "..", "config", "crd", "bases")},
		ErrorIfCRDPathMissing: true,
	}
	cfg, err := env.Start()
	if err != nil {
		t.Fatalf("start envtest: %v", err)
	}
	t.Cleanup(func() {
		if err := env.Stop(); err != nil {
			t.Errorf("stop envtest: %v", err)
		}
	})

	scheme := runtime.NewScheme()
	if err := corev1.AddToScheme(scheme); err != nil {
		t.Fatalf("add corev1: %v", err)
	}
	if err := runtimev1alpha1.AddToScheme(scheme); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	c, err := client.New(cfg, client.Options{Scheme: scheme})
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	return c, context.Background()
}

func createTestNamespace(t *testing.T, ctx context.Context, c client.Client, name string) string {
	t.Helper()
	if err := c.Create(ctx, &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{Name: name},
	}); err != nil {
		t.Fatalf("create namespace %q: %v", name, err)
	}
	return name
}

func TestHostStatusPatch_SatisfiesCRDRequired(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-status-test")

	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{Name: "envtest-host", Namespace: ns},
		HostID:     "0c6bc4e1-3781-43c7-afa0-58597dc41c58",
	}
	if err := c.Create(ctx, host); err != nil {
		t.Fatalf("create host: %v", err)
	}

	// host.Status is empty here, exactly as it is the first time the heartbeat
	// handler patches a host that reconcileReporting has not polled yet.
	patch, err := hostStatusPatch(&host.Status)
	if err != nil {
		t.Fatalf("hostStatusPatch: %v", err)
	}
	if err := c.Status().Patch(ctx, host, client.RawPatch(types.MergePatchType, patch)); err != nil {
		t.Fatalf("status patch rejected by API server (required-field regression?): %v", err)
	}

	got := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKeyFromObject(host), got); err != nil {
		t.Fatalf("get host: %v", err)
	}
	if got.Status.LastSeen.IsZero() {
		t.Errorf("expected lastSeen to be set after patch")
	}
	if got.Status.OSArch == "" || got.Status.Version == "" {
		t.Errorf("expected required string fields to be defaulted, got %+v", got.Status)
	}

	// A second patch after the host has reported real values must preserve them
	// (only lastSeen changes), mirroring steady-state heartbeats.
	got.Status.OSArch = testOSArch
	got.Status.Version = testVersion
	if err := c.Status().Update(ctx, got); err != nil {
		t.Fatalf("seed reported status: %v", err)
	}
	patch2, err := hostStatusPatch(&got.Status)
	if err != nil {
		t.Fatalf("hostStatusPatch (reported): %v", err)
	}
	if err := c.Status().Patch(ctx, got, client.RawPatch(types.MergePatchType, patch2)); err != nil {
		t.Fatalf("second status patch rejected: %v", err)
	}
	after := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKeyFromObject(host), after); err != nil {
		t.Fatalf("get host after second patch: %v", err)
	}
	if after.Status.OSArch != testOSArch || after.Status.Version != testVersion {
		t.Errorf("reported values clobbered by heartbeat patch: %+v", after.Status)
	}
}

// applyHost mirrors the heartbeat handler's Server-Side Apply upsert so the
// test exercises the exact create-or-update mechanics used in production.
func applyHost(ctx context.Context, c client.Client, ns, name, hostID, hostname string, httpPort uint32, env string, labels map[string]string) error {
	host := &runtimev1alpha1.Host{
		TypeMeta: metav1.TypeMeta{
			APIVersion: runtimev1alpha1.GroupVersion.String(),
			Kind:       "Host",
		},
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: ns,
			Labels:    labels,
		},
		HostID:      hostID,
		Hostname:    hostname,
		HTTPPort:    httpPort,
		Environment: env,
	}
	// Mirrors the production SSA path; client.Apply is deprecated but the
	// replacement needs a generated ApplyConfiguration the Host CRD lacks.
	return c.Patch(ctx, host, client.Apply, //nolint:staticcheck // matches host_controller.go
		client.FieldOwner("host-status-updater"), client.ForceOwnership)
}

// TestHostApply_UpsertIsIdempotent verifies the Server-Side Apply patch for the host information
// was applied correctly. It runs against a real API server so the apply is validated against
// the actual Host CRD schema.
func TestHostApply_UpsertIsIdempotent(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-apply-test")

	const name = "ssa-host"

	// First apply: object does not exist yet -> SSA creates it.
	if err := applyHost(ctx, c, ns, name, testHeartbeatHostID, "node-a", 8080, "tenant-a",
		map[string]string{"hostgroup": "default"}); err != nil {
		t.Fatalf("first apply (create) failed: %v", err)
	}

	created := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, created); err != nil {
		t.Fatalf("get after create: %v", err)
	}
	if created.HostID != testHeartbeatHostID || created.Hostname != "node-a" || created.HTTPPort != 8080 {
		t.Errorf("create did not persist spec/metadata: %+v", created)
	}

	// Second apply with changed fields: object exists -> SSA updates in place,
	// with no client-side Get and so no possibility of AlreadyExists.
	if err := applyHost(ctx, c, ns, name, testHeartbeatHostID, "node-a-renamed", 9090, "tenant-a",
		map[string]string{"hostgroup": "default"}); err != nil {
		t.Fatalf("second apply (update) failed: %v", err)
	}

	updated := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, updated); err != nil {
		t.Fatalf("get after update: %v", err)
	}
	if updated.Hostname != "node-a-renamed" || updated.HTTPPort != 9090 {
		t.Errorf("update did not persist changed fields: %+v", updated)
	}
	if updated.UID != created.UID {
		t.Errorf("update replaced the object (UID changed) instead of patching in place")
	}
}

// hostWith builds a Host carrying only the metadata spec fields that
// hostSpecChanged compares, so the heartbeat handler can skip a redundant
// Server-Side Apply when nothing changed.
func hostWith(hostID, hostname string, httpPort uint32, env string, labels map[string]string) *runtimev1alpha1.Host {
	return &runtimev1alpha1.Host{
		ObjectMeta:  metav1.ObjectMeta{Labels: labels},
		HostID:      hostID,
		Hostname:    hostname,
		HTTPPort:    httpPort,
		Environment: env,
	}
}

// TestHostSpecChanged covers every field the heartbeat handler diffs before
// deciding whether to re-apply the Host spec, including the nil-vs-empty label
// case that must not register as a change.
func TestHostSpecChanged(t *testing.T) {
	base := func() *runtimev1alpha1.Host {
		return hostWith(testHeartbeatHostID, "node-a", 8080, "tenant-a", map[string]string{"hostgroup": "default"})
	}

	tests := []struct {
		name string
		next *runtimev1alpha1.Host
		want bool
	}{
		{"identical", base(), false},
		{"hostID changed", hostWith("host-id-2", "node-a", 8080, "tenant-a", map[string]string{"hostgroup": "default"}), true},
		{"hostname changed", hostWith(testHeartbeatHostID, "node-b", 8080, "tenant-a", map[string]string{"hostgroup": "default"}), true},
		{"httpPort changed", hostWith(testHeartbeatHostID, "node-a", 9090, "tenant-a", map[string]string{"hostgroup": "default"}), true},
		{"environment changed", hostWith(testHeartbeatHostID, "node-a", 8080, "tenant-b", map[string]string{"hostgroup": "default"}), true},
		{"label value changed", hostWith(testHeartbeatHostID, "node-a", 8080, "tenant-a", map[string]string{"hostgroup": "other"}), true},
		{"label added", hostWith(testHeartbeatHostID, "node-a", 8080, "tenant-a", map[string]string{"hostgroup": "default", "extra": "x"}), true},
		{"label removed", hostWith(testHeartbeatHostID, "node-a", 8080, "tenant-a", nil), true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := hostSpecChanged(base(), tt.next); got != tt.want {
				t.Errorf("hostSpecChanged() = %v, want %v", got, tt.want)
			}
		})
	}

	// A nil label map and an empty one are equivalent and must not be reported
	// as a change: req.GetLabels() yields nil when the heartbeat carries none.
	t.Run("nil vs empty labels", func(t *testing.T) {
		existing := hostWith(testHeartbeatHostID, "node-a", 8080, "tenant-a", nil)
		next := hostWith(testHeartbeatHostID, "node-a", 8080, "tenant-a", map[string]string{})
		if hostSpecChanged(existing, next) {
			t.Errorf("nil and empty label maps must compare equal")
		}
	})
}

// mockBus is a wasmbus.Bus that answers Request with a canned reply (or error),
// recording the subject the host client addressed. Only Request is exercised by
// the heartbeat round-trip; the remaining methods satisfy the interface.
type mockBus struct {
	reply      *wasmbus.Message
	err        error
	gotSubject string
}

func (m *mockBus) Request(_ context.Context, msg *wasmbus.Message) (*wasmbus.Message, error) {
	m.gotSubject = msg.Subject
	if m.err != nil {
		return nil, m.err
	}
	return m.reply, nil
}

func (m *mockBus) Subscribe(string, int) (wasmbus.Subscription, error)              { return nil, nil }
func (m *mockBus) QueueSubscribe(string, string, int) (wasmbus.Subscription, error) { return nil, nil }
func (m *mockBus) Publish(*wasmbus.Message) error                                   { return nil }

// testUnreachableTimeout is the unreachable window the readiness tests judge
// hosts against. Long enough that no test trips it by wall-clock drift.
const testUnreachableTimeout = time.Hour

// settledFleetReconciler builds a HostReconciler that has been hearing the
// fleet without a gap for longer than the unreachable window, which is the
// steady state every host is judged in once the operator has been running for
// a while.
func settledFleetReconciler() *HostReconciler {
	r := &HostReconciler{UnreachableTimeout: testUnreachableTimeout}
	// Drive the witness the way production does: a run of heartbeats reaching
	// back past the window, the most recent of them just now.
	r.fleet.heard(time.Now().Add(-2 * testUnreachableTimeout))
	r.fleet.observe(time.Now().Add(-2*testUnreachableTimeout), testUnreachableTimeout)
	r.fleet.heard(time.Now())
	return r
}

// TestReconcileReporting verifies the heartbeat response is mapped onto Host
// status, including the float32->string CPU formatting and the unsigned->signed
// memory/count casts, and that the host's heartbeat subject is addressed.
func TestReconcileReporting(t *testing.T) {
	hb := &runtimev2.HostHeartbeat{
		SystemCpuUsage:    0.5, // exactly representable, so FormatFloat yields testCPUUsage
		SystemMemoryTotal: 16000,
		SystemMemoryFree:  8000,
		ComponentCount:    3,
		WorkloadCount:     2,
		OsName:            testOSName,
		OsArch:            testOSArch,
		OsKernel:          testOSKernel,
		Version:           testVersion,
	}
	data, err := protojson.Marshal(hb)
	if err != nil {
		t.Fatalf("marshal heartbeat: %v", err)
	}

	bus := &mockBus{reply: &wasmbus.Message{Data: data}}
	r := &HostReconciler{Bus: bus}
	host := &runtimev1alpha1.Host{HostID: testHeartbeatHostID}

	if err := r.reconcileReporting(context.Background(), host); err != nil {
		t.Fatalf("reconcileReporting: %v", err)
	}

	if want := "runtime.host.host-id-1.heartbeat"; bus.gotSubject != want {
		t.Errorf("heartbeat subject = %q, want %q", bus.gotSubject, want)
	}
	if host.Status.SystemCPUUsage != testCPUUsage {
		t.Errorf("SystemCPUUsage = %q, want %q", host.Status.SystemCPUUsage, testCPUUsage)
	}
	if host.Status.SystemMemoryTotal != 16000 {
		t.Errorf("SystemMemoryTotal = %d, want 16000", host.Status.SystemMemoryTotal)
	}
	if host.Status.SystemMemoryFree != 8000 {
		t.Errorf("SystemMemoryFree = %d, want 8000", host.Status.SystemMemoryFree)
	}
	if host.Status.ComponentCount != 3 || host.Status.WorkloadCount != 2 {
		t.Errorf("counts = (%d,%d), want (3,2)", host.Status.ComponentCount, host.Status.WorkloadCount)
	}
	if host.Status.OSName != testOSName || host.Status.OSArch != testOSArch ||
		host.Status.OSKernel != testOSKernel || host.Status.Version != testVersion {
		t.Errorf("OS/version fields not mapped: %+v", host.Status)
	}
	if host.Status.LastSeen.IsZero() {
		t.Errorf("LastSeen must be set after a successful heartbeat")
	}
}

// TestReconcileReporting_Error verifies a transport error from the bus is
// surfaced and no status fields are written.
func TestReconcileReporting_Error(t *testing.T) {
	bus := &mockBus{err: errors.New("bus down")}
	r := &HostReconciler{Bus: bus}
	host := &runtimev1alpha1.Host{HostID: testHeartbeatHostID}

	if err := r.reconcileReporting(context.Background(), host); err == nil {
		t.Fatalf("expected error when the heartbeat request fails")
	}
	if !host.Status.LastSeen.IsZero() {
		t.Errorf("LastSeen must not be written when the heartbeat fails")
	}
}

// TestReconcileReady covers the three readiness branches: reporting-true is
// ready, a recent-but-not-reporting host is Unknown (still within the
// unreachable window), and a long-silent host is a definite failure.
func TestReconcileReady(t *testing.T) {
	t.Run("reporting true is ready", func(t *testing.T) {
		r := settledFleetReconciler()
		host := &runtimev1alpha1.Host{}
		host.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.HostConditionReporting))
		if err := r.reconcileReady(context.Background(), host); err != nil {
			t.Errorf("expected nil error when Reporting is true, got %v", err)
		}
	})

	t.Run("recently seen but not reporting is unknown", func(t *testing.T) {
		r := settledFleetReconciler()
		host := &runtimev1alpha1.Host{}
		host.Status.LastSeen = metav1.Now()
		err := r.reconcileReady(context.Background(), host)
		if err == nil {
			t.Fatalf("expected an error while not reporting")
		}
		if !condition.IsStatusUnknown(err) {
			t.Errorf("within the unreachable window the error must be ErrStatusUnknown, got %v", err)
		}
	})

	t.Run("silent past the unreachable window is failed", func(t *testing.T) {
		r := settledFleetReconciler()
		host := &runtimev1alpha1.Host{}
		host.Status.LastSeen = metav1.NewTime(time.Now().Add(-2 * testUnreachableTimeout))
		err := r.reconcileReady(context.Background(), host)
		if err == nil {
			t.Fatalf("expected an error for a long-silent host")
		}
		if condition.IsStatusUnknown(err) {
			t.Errorf("past the unreachable window the error must be a definite failure, not Unknown: %v", err)
		}
	})
}

// TestReconcileReporting_StartsFleetGraceClock pins the grace clock to a call
// site that runs on every pass. Wound only where it is read — the failure path
// — it would read zero through a healthy uptime.
func TestReconcileReporting_StartsFleetGraceClock(t *testing.T) {
	r := &HostReconciler{
		Bus:                &mockBus{err: errors.New("no responders")},
		UnreachableTimeout: testUnreachableTimeout,
	}
	// Some host was heard from, so the operator is not deaf — this one host's
	// probe just failed.
	start := time.Now()
	r.fleet.heard(start)

	if err := r.reconcileReporting(context.Background(), &runtimev1alpha1.Host{HostID: testHeartbeatHostID}); err == nil {
		t.Fatalf("expected the heartbeat probe to fail")
	}

	if r.fleet.settled(start, testUnreachableTimeout) {
		t.Errorf("the window cannot already be satisfied the instant the clock starts")
	}

	// Well past the window, with the fleet still being heard.
	later := start.Add(2 * testUnreachableTimeout)
	r.fleet.heard(later)
	if !r.fleet.settled(later, testUnreachableTimeout) {
		t.Errorf("the window must be satisfied once the fleet has been heard for it")
	}
}

// TestFleetWitness_SilenceDuringProbe covers the gap between the two calls: a
// pass observes while the fleet is still inside the window, then spends up to
// hostHeartbeatTimeout in the probe, by which point it is not. settled has to
// re-check arrival rather than trust that observation.
func TestFleetWitness_SilenceDuringProbe(t *testing.T) {
	var w fleetWitness

	busDied := time.Now()
	w.heard(busDied)
	w.heardSince = busDied.Add(-2 * time.Hour)

	observeAt := busDied.Add(testUnreachableTimeout - hostHeartbeatTimeout + time.Second)
	w.observe(observeAt, testUnreachableTimeout)

	settledAt := observeAt.Add(hostHeartbeatTimeout)
	if w.settled(settledAt, testUnreachableTimeout) {
		t.Errorf("settled with the last heartbeat %v ago, past the %v window",
			settledAt.Sub(busDied), testUnreachableTimeout)
	}
}

// TestReconcileReady_FleetUnheard pins the guard that keeps the operator's own
// deafness from being charged to the hosts, whose Workloads a definite
// not-ready verdict deletes.
func TestReconcileReady_FleetUnheard(t *testing.T) {
	silentHost := func() *runtimev1alpha1.Host {
		host := &runtimev1alpha1.Host{}
		host.Status.LastSeen = metav1.NewTime(time.Now().Add(-2 * testUnreachableTimeout))
		return host
	}
	observe := func(r *HostReconciler) {
		r.fleet.observe(time.Now(), r.UnreachableTimeout)
	}

	t.Run("fleet heard past the window still fails the host", func(t *testing.T) {
		// The counterweight to every case below: a host that really is gone
		// must still be reaped, or its Workloads are never rescheduled.
		r := settledFleetReconciler()
		observe(r)
		err := r.reconcileReady(context.Background(), silentHost())
		if err == nil {
			t.Fatalf("expected an error for a long-silent host")
		}
		if condition.IsStatusUnknown(err) {
			t.Errorf("a dead host must be a definite failure once the fleet has been heard a full window, got %v", err)
		}
	})

	t.Run("unheard fleet holds the host at unknown", func(t *testing.T) {
		// A dead fleet and a deaf operator are indistinguishable here, and
		// holding costs nothing: a fleet nobody can hear has nowhere to
		// reschedule to either.
		r := settledFleetReconciler()
		r.fleet.lastHeard = time.Now().Add(-2 * testUnreachableTimeout)
		observe(r)
		err := r.reconcileReady(context.Background(), silentHost())
		if err == nil {
			t.Fatalf("expected an error for a long-silent host")
		}
		if !condition.IsStatusUnknown(err) {
			t.Errorf("a silent host must stay Unknown while the operator hears nothing, got %v", err)
		}
	})

	t.Run("freshly started operator holds the host at unknown", func(t *testing.T) {
		// No priming: nothing heard yet, exactly as on the first reconcile
		// after the process starts with LastSeen timestamps that went stale
		// while it was down.
		r := &HostReconciler{Bus: &mockBus{}, UnreachableTimeout: testUnreachableTimeout}
		observe(r)
		err := r.reconcileReady(context.Background(), silentHost())
		if err == nil {
			t.Fatalf("expected an error for a long-silent host")
		}
		if !condition.IsStatusUnknown(err) {
			t.Errorf("a silent host must stay Unknown until the fleet has been heard a full window, got %v", err)
		}
	})

	t.Run("hearing the fleet again restarts the grace period", func(t *testing.T) {
		// Why arrival alone cannot be the gate: hosts publish every 15s, so
		// just after the bus returns one has ticked and its neighbours have
		// not, and they would be condemned on the strength of the one.
		r := settledFleetReconciler()
		r.fleet.lastHeard = time.Now().Add(-2 * testUnreachableTimeout)
		observe(r)
		if err := r.reconcileReady(context.Background(), silentHost()); !condition.IsStatusUnknown(err) {
			t.Fatalf("expected Unknown while the operator hears nothing, got %v", err)
		}

		r.fleet.heard(time.Now())
		observe(r)
		err := r.reconcileReady(context.Background(), silentHost())
		if !condition.IsStatusUnknown(err) {
			t.Errorf("the window must restart from the first heartbeat back, not carry over, got %v", err)
		}
	})
}

// TestHeartbeatHandler_WitnessesFleet pins the wiring the whole guard rests on:
// the readiness verdict is gated on heartbeats having arrived, so the handler
// they arrive at has to record that they did.
func TestHeartbeatHandler_WitnessesFleet(t *testing.T) {
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}

	var fleet fleetWitness
	// A fake client cannot serve the Server-Side Apply this handler makes, so
	// the Host bookkeeping below fails — which is the point. Arrival is the
	// evidence, and it has to be recorded before anything that can fail.
	updater := &hostStatusUpdater{
		client:            fake.NewClientBuilder().WithScheme(s).Build(),
		operatorNamespace: testNamespace,
		fleet:             &fleet,
	}

	start := time.Now()
	if fleet.settled(start, testUnreachableTimeout) {
		t.Fatalf("nothing heard yet, so the fleet cannot be settled")
	}

	updater.handleHeartbeat(context.Background(), logr.Discard(),
		heartbeatBytes(t, "hb-host", testHeartbeatHostID, "node-a"))

	// The handler's stamp is what lets a run start here; without it observe
	// finds nothing heard and the witness never settles.
	fleet.observe(start, testUnreachableTimeout)

	later := start.Add(2 * testUnreachableTimeout)
	fleet.heard(later)
	if !fleet.settled(later, testUnreachableTimeout) {
		t.Errorf("an arrived heartbeat must be recorded as hearing the fleet")
	}
}

// newHostFinalizeClient builds a fake client wired with the same Status.HostID
// index finalize relies on in production, seeded with the given objects.
func newHostFinalizeClient(t *testing.T, objs ...client.Object) client.Client {
	t.Helper()
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	return fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.Workload{}, workloadByHostIDIndex,
			func(obj client.Object) []string {
				workload, ok := obj.(*runtimev1alpha1.Workload)
				if !ok || workload.Status.HostID == "" {
					return nil
				}
				return []string{workload.Status.HostID}
			}).
		Build()
}

func workloadForHost(name, hostID string) *runtimev1alpha1.Workload {
	w := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: "tenant-a"},
	}
	w.Status.HostID = hostID
	return w
}

// TestFinalize verifies the host finalizer deletes exactly the live workloads
// assigned to it: workloads on other hosts are untouched, and a workload
// already being deleted is left alone rather than re-deleted.
func TestFinalize(t *testing.T) {
	assigned := workloadForHost("assigned", testHeartbeatHostID)

	deleting := workloadForHost("deleting", testHeartbeatHostID)
	now := metav1.Now()
	deleting.DeletionTimestamp = &now
	// A fake-client object carrying a DeletionTimestamp must also carry a
	// finalizer, otherwise the builder rejects it; the finalizer also keeps it
	// present so we can assert finalize skipped it.
	deleting.Finalizers = []string{"runtime.wasmcloud.dev/test-hold"}

	otherHost := workloadForHost("other-host", "host-id-2")

	c := newHostFinalizeClient(t, assigned, deleting, otherHost)
	r := &HostReconciler{Client: c}
	host := &runtimev1alpha1.Host{HostID: testHeartbeatHostID}

	if err := r.finalize(context.Background(), host); err != nil {
		t.Fatalf("finalize: %v", err)
	}

	if err := c.Get(context.Background(), client.ObjectKeyFromObject(assigned), &runtimev1alpha1.Workload{}); !apierrors.IsNotFound(err) {
		t.Errorf("assigned workload should have been deleted, got err=%v", err)
	}
	if err := c.Get(context.Background(), client.ObjectKeyFromObject(deleting), &runtimev1alpha1.Workload{}); err != nil {
		t.Errorf("already-deleting workload must be left in place, got err=%v", err)
	}
	if err := c.Get(context.Background(), client.ObjectKeyFromObject(otherHost), &runtimev1alpha1.Workload{}); err != nil {
		t.Errorf("workload on another host must be untouched, got err=%v", err)
	}
}

// newHostDeleteClient builds a fake client seeded with a single Host (no
// finalizer, so a Delete fully removes it and the assertion is unambiguous).
func newHostDeleteClient(t *testing.T, host *runtimev1alpha1.Host) (client.Client, *runtimev1alpha1.Host) {
	t.Helper()
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	host.Name = "host-a"
	host.Namespace = "wasmcloud-system"
	c := fake.NewClientBuilder().WithScheme(s).WithObjects(host).Build()
	return c, host
}

// TestDeleteUnresponsiveHost pins the post-hook contract: a Ready host and a
// host whose Ready is still Unknown are both kept, while a host whose Ready has
// resolved to a definite failure is deleted so the finalizer can run.
func TestDeleteUnresponsiveHost(t *testing.T) {
	t.Run("ready host is kept", func(t *testing.T) {
		host := &runtimev1alpha1.Host{}
		host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
		c, h := newHostDeleteClient(t, host)
		r := &HostReconciler{Client: c}
		if err := r.deleteUnresponsiveHost(context.Background(), h); err != nil {
			t.Fatalf("deleteUnresponsiveHost: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(h), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("ready host must not be deleted, got err=%v", err)
		}
	})

	t.Run("unknown host is kept", func(t *testing.T) {
		host := &runtimev1alpha1.Host{}
		host.Status.SetConditions(condition.UnknownCondition(condition.TypeReady, condition.ReasonUnavailable, "not reporting"))
		c, h := newHostDeleteClient(t, host)
		r := &HostReconciler{Client: c}
		if err := r.deleteUnresponsiveHost(context.Background(), h); err != nil {
			t.Fatalf("deleteUnresponsiveHost: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(h), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("host with Unknown Ready must not be deleted, got err=%v", err)
		}
	})

	t.Run("failed host is deleted", func(t *testing.T) {
		host := &runtimev1alpha1.Host{}
		host.Status.SetConditions(condition.ErrorCondition(condition.TypeReady, condition.ReasonUnavailable, errors.New("host down")))
		c, h := newHostDeleteClient(t, host)
		r := &HostReconciler{Client: c}
		if err := r.deleteUnresponsiveHost(context.Background(), h); err != nil {
			t.Fatalf("deleteUnresponsiveHost: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(h), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
			t.Errorf("definitely-not-ready host must be deleted, got err=%v", err)
		}
	})
}

// applyCountingClient wraps a client.Client and counts Server-Side Apply
// patches (the spec upsert), so a test can assert whether the heartbeat handler
// performed or skipped the apply. Status subresource patches go through
// Status() and are not counted.
type applyCountingClient struct {
	client.Client
	applies atomic.Int64
}

func (c *applyCountingClient) Patch(ctx context.Context, obj client.Object, patch client.Patch, opts ...client.PatchOption) error {
	if patch.Type() == types.ApplyPatchType {
		c.applies.Add(1)
	}
	return c.Client.Patch(ctx, obj, patch, opts...)
}

func heartbeatBytes(t *testing.T, name, hostID, hostname string) []byte {
	t.Helper()
	data, err := protojson.Marshal(&runtimev2.HostHeartbeat{
		FriendlyName: name,
		Id:           hostID,
		Hostname:     hostname,
	})
	if err != nil {
		t.Fatalf("marshal heartbeat: %v", err)
	}
	return data
}

// TestHeartbeatHandler_SkipsRedundantApply exercises the full handler path
// against a real API server: the first heartbeat creates the Host (one apply),
// an identical heartbeat is skipped (no apply), and a changed heartbeat applies
// again. LastSeen is refreshed on every heartbeat regardless of the skip.
func TestHeartbeatHandler_SkipsRedundantApply(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-heartbeat-skip")

	counter := &applyCountingClient{Client: c}
	updater := &hostStatusUpdater{client: counter, operatorNamespace: ns, fleet: &fleetWitness{}}
	log := logr.Discard()

	const name = "hb-host"

	// First heartbeat: object does not exist -> create via SSA.
	updater.handleHeartbeat(ctx, log, heartbeatBytes(t, name, testHeartbeatHostID, "node-a"))
	if got := counter.applies.Load(); got != 1 {
		t.Fatalf("first heartbeat: applies = %d, want 1", got)
	}
	created := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, created); err != nil {
		t.Fatalf("get after first heartbeat: %v", err)
	}
	if created.Hostname != "node-a" {
		t.Errorf("hostname = %q, want node-a", created.Hostname)
	}
	if created.Status.LastSeen.IsZero() {
		t.Errorf("LastSeen must be set after the first heartbeat")
	}

	// Identical heartbeat: spec unchanged -> no apply, but LastSeen still refreshes.
	updater.handleHeartbeat(ctx, log, heartbeatBytes(t, name, testHeartbeatHostID, "node-a"))
	if got := counter.applies.Load(); got != 1 {
		t.Fatalf("redundant heartbeat must not re-apply: applies = %d, want 1", got)
	}
	afterRedundant := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, afterRedundant); err != nil {
		t.Fatalf("get after redundant heartbeat: %v", err)
	}
	if afterRedundant.Status.LastSeen.IsZero() {
		t.Errorf("LastSeen must remain set after a redundant heartbeat")
	}

	// Changed heartbeat: hostname differs -> apply again.
	updater.handleHeartbeat(ctx, log, heartbeatBytes(t, name, testHeartbeatHostID, "node-b"))
	if got := counter.applies.Load(); got != 2 {
		t.Fatalf("changed heartbeat must re-apply: applies = %d, want 2", got)
	}
	updated := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, updated); err != nil {
		t.Fatalf("get after changed heartbeat: %v", err)
	}
	if updated.Hostname != "node-b" {
		t.Errorf("hostname = %q, want node-b after change", updated.Hostname)
	}

	// A restarted host that reuses its friendly name reports a new host ID, and
	// the operator addresses every RPC by that ID — so this is the one field the
	// skip must never swallow.
	updater.handleHeartbeat(ctx, log, heartbeatBytes(t, name, "host-id-2", "node-b"))
	if got := counter.applies.Load(); got != 3 {
		t.Fatalf("changed hostID must re-apply: applies = %d, want 3", got)
	}
	reidentified := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, reidentified); err != nil {
		t.Fatalf("get after hostID change: %v", err)
	}
	if reidentified.HostID != "host-id-2" {
		t.Errorf("hostID = %q, want host-id-2 after change", reidentified.HostID)
	}
}

// TestHeartbeatHandler_PreservesReportedStatus is the call-site counterpart to
// TestHostStatusPatch_ReportedFieldsPreserved: the placeholders must come from
// the stored status, so a heartbeat leaves what reconcileReporting measured.
func TestHeartbeatHandler_PreservesReportedStatus(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-heartbeat-status")

	updater := &hostStatusUpdater{client: c, operatorNamespace: ns, fleet: &fleetWitness{}}
	log := logr.Discard()

	const name = "hb-status-host"
	updater.handleHeartbeat(ctx, log, heartbeatBytes(t, name, testHeartbeatHostID, "node-a"))

	// Stand in for reconcileReporting having polled the host.
	reported := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, reported); err != nil {
		t.Fatalf("get after first heartbeat: %v", err)
	}
	reported.Status.Version = testVersion
	reported.Status.OSName = testOSName
	reported.Status.OSArch = testOSArch
	reported.Status.OSKernel = testOSKernel
	reported.Status.SystemCPUUsage = testCPUUsage
	reported.Status.SystemMemoryTotal = 16000
	reported.Status.SystemMemoryFree = 8000
	if err := c.Status().Update(ctx, reported); err != nil {
		t.Fatalf("seed reported status: %v", err)
	}
	seenBefore := reported.Status.LastSeen

	updater.handleHeartbeat(ctx, log, heartbeatBytes(t, name, testHeartbeatHostID, "node-a"))

	after := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, after); err != nil {
		t.Fatalf("get after steady-state heartbeat: %v", err)
	}
	if after.Status.Version != testVersion || after.Status.OSName != testOSName ||
		after.Status.OSArch != testOSArch || after.Status.OSKernel != testOSKernel {
		t.Errorf("heartbeat clobbered reported OS/version fields: %+v", after.Status)
	}
	if after.Status.SystemCPUUsage != testCPUUsage ||
		after.Status.SystemMemoryTotal != 16000 || after.Status.SystemMemoryFree != 8000 {
		t.Errorf("heartbeat clobbered reported system metrics: %+v", after.Status)
	}
	if !after.Status.LastSeen.After(seenBefore.Time) && !after.Status.LastSeen.Equal(&seenBefore) {
		t.Errorf("LastSeen went backwards: %v -> %v", seenBefore, after.Status.LastSeen)
	}
}
