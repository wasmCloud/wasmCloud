package runtime

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
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

// TestHostStatusPatch_EmptyStatusInjectsRequiredKeys guards the regression: a
// status write before reconcileReporting has polled the host must still carry
// every CRD-required key, plus lastSeen. A diff-based MergeFrom patch used to
// drop value-equal fields (notably int64 0), leaving the keys absent and the
// status subresource rejected with "Required value".
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
		Version:           "1.2.3",
		OSName:            "linux",
		OSArch:            "arm64",
		OSKernel:          "6.1.0",
		SystemCPUUsage:    "0.5",
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
// API server with the Host CRD installed. It reproduces the production failure
// path: a freshly created Host whose status has not yet been populated by
// reconcileReporting, then the heartbeat handler's status patch. Before the
// fix, the API server rejected this with "status.osArch: Required value" (and
// the other required fields). Requires envtest binaries — skipped when
// KUBEBUILDER_ASSETS is unset (e.g. plain `go test`); `make test` sets it.
// startHostEnvtest boots an envtest API server with the operator's CRDs
// installed and returns a client plus a context. The environment is torn down
// via t.Cleanup. Tests are skipped when KUBEBUILDER_ASSETS is unset (e.g. plain
// `go test`); `make test` sets it.
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
	got.Status.OSArch = "arm64"
	got.Status.Version = "1.2.3"
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
	if after.Status.OSArch != "arm64" || after.Status.Version != "1.2.3" {
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
	return c.Patch(ctx, host, client.Apply,
		client.FieldOwner("host-status-updater"), client.ForceOwnership)
}

// TestHostApply_UpsertIsIdempotent verifies the Server-Side Apply path that
// replaced CreateOrUpdate+retry: the first apply creates the Host, a second
// apply updates it in place, and neither can produce an AlreadyExists error
// (the failure mode SSA was adopted to eliminate). It runs against a real API
// server so the apply is validated against the actual Host CRD schema.
func TestHostApply_UpsertIsIdempotent(t *testing.T) {
	c, ctx := startHostEnvtest(t)
	ns := createTestNamespace(t, ctx, c, "host-apply-test")

	const name = "ssa-host"

	// First apply: object does not exist yet -> SSA creates it.
	if err := applyHost(ctx, c, ns, name, "host-id-1", "node-a", 8080, "tenant-a",
		map[string]string{"hostgroup": "default"}); err != nil {
		t.Fatalf("first apply (create) failed: %v", err)
	}

	created := &runtimev1alpha1.Host{}
	if err := c.Get(ctx, client.ObjectKey{Namespace: ns, Name: name}, created); err != nil {
		t.Fatalf("get after create: %v", err)
	}
	if created.HostID != "host-id-1" || created.Hostname != "node-a" || created.HTTPPort != 8080 {
		t.Errorf("create did not persist spec/metadata: %+v", created)
	}

	// Second apply with changed fields: object exists -> SSA updates in place,
	// with no client-side Get and so no possibility of AlreadyExists.
	if err := applyHost(ctx, c, ns, name, "host-id-1", "node-a-renamed", 9090, "tenant-a",
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
