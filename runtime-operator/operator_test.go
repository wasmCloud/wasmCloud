package runtime_operator

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/go-logr/logr"
	"github.com/go-logr/logr/funcr"
	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats.go"
	"google.golang.org/protobuf/encoding/protojson"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/discovery"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	"k8s.io/client-go/rest"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/config"
	"sigs.k8s.io/controller-runtime/pkg/envtest"
	"sigs.k8s.io/controller-runtime/pkg/manager"
	metricsserver "sigs.k8s.io/controller-runtime/pkg/metrics/server"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimectrl "go.wasmcloud.dev/runtime-operator/v2/internal/controller/runtime"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

const (
	registeredName        = "registered"
	testOperatorNamespace = "operator-system"
	testTenantNamespace   = "tenant"
	podFinalizer          = "runtime.wasmcloud.dev/pod-host-finalizer"
)

var daemonSetCRDPath = filepath.Join("config", "crd", "bases", "runtime.wasmcloud.dev_workloaddaemonsets.yaml")

var shared struct {
	once   sync.Once
	env    *envtest.Environment
	cfg    *rest.Config
	k8s    client.Client
	scheme *runtime.Scheme
	err    error
}

func TestMain(m *testing.M) {
	code := m.Run()
	if shared.env != nil {
		if err := shared.env.Stop(); err != nil {
			fmt.Fprintf(os.Stderr, "stop envtest: %v\n", err)
			code = 1
		}
	}
	os.Exit(code)
}

// sharedEnvtest starts one API server for the package on first use, with the
// operator's namespace and one tenant namespace.
func sharedEnvtest(t *testing.T) (*rest.Config, client.Client) {
	t.Helper()
	if os.Getenv("KUBEBUILDER_ASSETS") == "" {
		t.Skip("KUBEBUILDER_ASSETS not set; run via `make test` for envtest")
	}
	shared.once.Do(func() {
		shared.scheme = runtime.NewScheme()
		if shared.err = clientgoscheme.AddToScheme(shared.scheme); shared.err != nil {
			return
		}
		if shared.err = runtimev1alpha1.AddToScheme(shared.scheme); shared.err != nil {
			return
		}
		shared.env = &envtest.Environment{
			CRDDirectoryPaths:     []string{filepath.Join("config", "crd", "bases")},
			ErrorIfCRDPathMissing: true,
		}
		if shared.cfg, shared.err = shared.env.Start(); shared.err != nil {
			return
		}
		if shared.k8s, shared.err = client.New(shared.cfg, client.Options{Scheme: shared.scheme}); shared.err != nil {
			return
		}
		for _, ns := range []string{testOperatorNamespace, testTenantNamespace} {
			if shared.err = shared.k8s.Create(context.Background(),
				&corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: ns}}); shared.err != nil {
				return
			}
		}
	})
	if shared.err != nil {
		t.Fatalf("start envtest: %v", shared.err)
	}
	return shared.cfg, shared.k8s
}

// newManager builds a manager for cfg that logs to log. Controller names
// repeat across this package's managers, so name validation is off.
func newManager(t *testing.T, cfg *rest.Config, log logr.Logger) manager.Manager {
	t.Helper()
	mgr, err := ctrl.NewManager(cfg, ctrl.Options{
		Scheme:                 shared.scheme,
		Logger:                 log,
		Metrics:                metricsserver.Options{BindAddress: "0"},
		HealthProbeBindAddress: "0",
		Controller:             config.Controller{SkipNameValidation: new(true)},
	})
	if err != nil {
		t.Fatalf("new manager: %v", err)
	}
	return mgr
}

// startManager runs mgr until the test ends, failing the test if it stops
// with an error.
func startManager(t *testing.T, mgr manager.Manager) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- mgr.Start(ctx) }()
	t.Cleanup(func() {
		cancel()
		select {
		case err := <-done:
			if err != nil {
				t.Errorf("manager stopped with %v", err)
			}
		case <-time.After(30 * time.Second):
			t.Error("manager did not stop")
		}
	})
	if !mgr.GetCache().WaitForCacheSync(ctx) {
		t.Fatal("manager cache did not sync")
	}
}

// logRecorder keeps every line a logr.Logger writes.
type logRecorder struct {
	mu    sync.Mutex
	lines []string
}

func (r *logRecorder) logger() logr.Logger {
	return funcr.New(func(prefix, args string) {
		r.mu.Lock()
		defer r.mu.Unlock()
		r.lines = append(r.lines, prefix+" "+args)
	}, funcr.Options{Verbosity: 1})
}

func (r *logRecorder) matching(substr string) []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	var got []string
	for _, line := range r.lines {
		if strings.Contains(line, substr) {
			got = append(got, line)
		}
	}
	return got
}

func startNats(t *testing.T) *server.Server {
	t.Helper()
	s, err := server.NewServer(&server.Options{Host: "127.0.0.1", Port: server.RANDOM_PORT, NoLog: true, NoSigs: true})
	if err != nil {
		t.Fatalf("new nats server: %v", err)
	}
	s.Start()
	if !s.ReadyForConnections(10 * time.Second) {
		s.Shutdown()
		t.Fatal("nats server did not become ready")
	}
	t.Cleanup(func() {
		s.Shutdown()
		s.WaitForShutdown()
	})
	return s
}

func operatorConfig(natsURL string) EmbeddedOperatorConfig {
	return EmbeddedOperatorConfig{NatsURL: natsURL, Namespace: testOperatorNamespace, AllowSharedHosts: true}
}

// newOperator builds the operator on mgr and closes its NATS connection when
// the test ends.
func newOperator(t *testing.T, mgr manager.Manager, cfg EmbeddedOperatorConfig) *EmbeddedOperator {
	t.Helper()
	op, err := NewEmbeddedOperator(context.Background(), mgr, cfg)
	if err != nil {
		t.Fatalf("NewEmbeddedOperator: %v", err)
	}
	t.Cleanup(op.NatsConn.Close)
	return op
}

func eventually(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(30 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(100 * time.Millisecond)
	}
}

func get(t *testing.T, k8s client.Client, key client.ObjectKey, obj client.Object) bool {
	t.Helper()
	err := k8s.Get(context.Background(), key, obj)
	if apierrors.IsNotFound(err) {
		return false
	}
	if err != nil {
		t.Fatalf("get %T %s: %v", obj, key, err)
	}
	return true
}

func create(t *testing.T, k8s client.Client, obj client.Object) {
	t.Helper()
	if err := k8s.Create(context.Background(), obj); err != nil {
		t.Fatalf("create %T %s: %v", obj, obj.GetName(), err)
	}
	t.Cleanup(func() { remove(t, k8s, obj) })
}

// remove deletes obj and then clears its finalizers, which nothing releases
// once the test's operator stops. A deleting object takes no new finalizers,
// so a running operator cannot put them back.
func remove(t *testing.T, k8s client.Client, obj client.Object) {
	t.Helper()
	ctx := context.Background()
	if err := k8s.Delete(ctx, obj); client.IgnoreNotFound(err) != nil {
		t.Errorf("delete %T %s: %v", obj, obj.GetName(), err)
	}
	release := client.RawPatch(types.MergePatchType, []byte(`{"metadata":{"finalizers":null}}`))
	if err := k8s.Patch(ctx, obj, release); client.IgnoreNotFound(err) != nil {
		t.Errorf("release %T %s: %v", obj, obj.GetName(), err)
	}
}

func newArtifact(name string) *runtimev1alpha1.Artifact {
	return &runtimev1alpha1.Artifact{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: testTenantNamespace},
		Spec:       runtimev1alpha1.ArtifactSpec{Image: "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0"},
	}
}

func newHostPod(name string) *corev1.Pod {
	return &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: name, Namespace: testOperatorNamespace,
			Labels: map[string]string{runtimectrl.HostPodLabel: "default"},
		},
		Spec: corev1.PodSpec{Containers: []corev1.Container{{Name: "host", Image: "wasmcloud/wash"}}},
	}
}

func workloadTemplate() runtimev1alpha1.WorkloadReplicaTemplate {
	return runtimev1alpha1.WorkloadReplicaTemplate{Spec: runtimev1alpha1.WorkloadSpec{
		Components: []runtimev1alpha1.WorkloadComponent{{Name: "http", Image: "ghcr.io/wasmcloud/components/http:0.1.0"}},
	}}
}

func artifactPublished(t *testing.T, k8s client.Client, artifact *runtimev1alpha1.Artifact) bool {
	t.Helper()
	var current runtimev1alpha1.Artifact
	return get(t, k8s, client.ObjectKeyFromObject(artifact), &current) && current.Status.ArtifactURL != ""
}

func podHasFinalizer(t *testing.T, k8s client.Client, pod *corev1.Pod) bool {
	t.Helper()
	var current corev1.Pod
	return get(t, k8s, client.ObjectKeyFromObject(pod), &current) && slices.Contains(current.Finalizers, podFinalizer)
}

// controlledBy returns the item of items that owner controls, or nil.
func controlledBy[T any, PT interface {
	*T
	client.Object
}](items []T, owner client.Object) PT {
	for i := range items {
		item := PT(&items[i])
		if ref := metav1.GetControllerOf(item); ref != nil && ref.UID == owner.GetUID() {
			return item
		}
	}
	return nil
}

func TestNewEmbeddedOperatorRequiresNamespaceBeforeConnecting(t *testing.T) {
	s := startNats(t)
	cfg := operatorConfig(s.ClientURL())
	cfg.Namespace = ""

	op, err := NewEmbeddedOperator(context.Background(), nil, cfg)
	if err == nil || !strings.Contains(err.Error(), "Namespace is required") {
		t.Fatalf("NewEmbeddedOperator() = %v, %v; want a missing Namespace error", op, err)
	}
	if clients := s.NumClients(); clients != 0 {
		t.Errorf("NATS saw %d clients; validation must come before connecting", clients)
	}
}

func TestNewEmbeddedOperatorReportsNATSFailure(t *testing.T) {
	cfg, _ := sharedEnvtest(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	op, err := NewEmbeddedOperator(ctx, newManager(t, cfg, logr.Discard()), operatorConfig("nats://127.0.0.1:1"))
	if !errors.Is(err, wasmbus.ErrTransport) {
		t.Fatalf("NewEmbeddedOperator() = %v, %v; want a transport error", op, err)
	}
}

// The TTL is read back from a silent Host: before the operator has heard the
// fleet for a full TTL, its Ready message names the window it is waiting out.
func TestNewEmbeddedOperatorClampsHeartbeatTTL(t *testing.T) {
	cfg, k8s := sharedEnvtest(t)
	s := startNats(t)
	for i, tt := range []struct {
		name       string
		ttl, using time.Duration
	}{
		{name: "unset uses the default", using: DefaultHeartbeatTTL},
		{name: "below the minimum is raised", ttl: MinHeartbeatTTL / 3, using: MinHeartbeatTTL},
		{name: "at the minimum is kept", ttl: MinHeartbeatTTL, using: MinHeartbeatTTL},
		{name: "above the minimum is kept", ttl: DefaultHeartbeatTTL * 2, using: DefaultHeartbeatTTL * 2},
	} {
		t.Run(tt.name, func(t *testing.T) {
			logs := &logRecorder{}
			mgr := newManager(t, cfg, logs.logger())
			operator := operatorConfig(s.ClientURL())
			operator.HeartbeatTTL = tt.ttl
			newOperator(t, mgr, operator)

			raised := logs.matching("HeartbeatTTL raised")
			switch tt.ttl {
			case 0, tt.using:
				if len(raised) != 0 {
					t.Errorf("TTL %s was raised: %v", tt.ttl, raised)
				}
			default:
				want := fmt.Sprintf(`"configured"=%q "using"=%q`, tt.ttl, tt.using)
				if len(raised) != 1 || !strings.Contains(raised[0], want) {
					t.Errorf("raise log = %v, want one line with %s", raised, want)
				}
			}

			startManager(t, mgr)
			name := fmt.Sprintf("silent-%d", i)
			host := &runtimev1alpha1.Host{
				ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: testOperatorNamespace},
				HostID:     name,
			}
			create(t, k8s, host)
			var message string
			eventually(t, "the silent Host's Ready message", func() bool {
				var current runtimev1alpha1.Host
				if get(t, k8s, client.ObjectKeyFromObject(host), &current) {
					message = current.Status.GetCondition(condition.TypeReady).Message
				}
				return strings.Contains(message, "hearing the fleet for")
			})
			if want := "hearing the fleet for " + tt.using.String(); !strings.HasSuffix(message, want) {
				t.Errorf("Ready message = %q, want it to end %q", message, want)
			}
		})
	}
}

// TestNewEmbeddedOperatorRegistersControllers starts the embedded operator and
// gives every controller something to do. Each depends on the field indexes
// its setup registers: a List against a missing index fails the reconcile.
func TestNewEmbeddedOperatorRegistersControllers(t *testing.T) {
	cfg, k8s := sharedEnvtest(t)
	s := startNats(t)
	mgr := newManager(t, cfg, logr.Discard())
	newOperator(t, mgr, operatorConfig(s.ClientURL()))
	startManager(t, mgr)

	artifact := newArtifact(registeredName)
	create(t, k8s, artifact)
	eventually(t, "the Artifact to be published", func() bool { return artifactPublished(t, k8s, artifact) })

	pod := newHostPod(registeredName)
	create(t, k8s, pod)
	eventually(t, "the host Pod's finalizer", func() bool { return podHasFinalizer(t, k8s, pod) })

	nc, err := nats.Connect(s.ClientURL())
	if err != nil {
		t.Fatalf("connect to nats: %v", err)
	}
	defer nc.Close()
	heartbeat, err := protojson.Marshal(&runtimev2.HostHeartbeat{
		Id: registeredName + "-id", FriendlyName: registeredName, Hostname: "10.1.0.1",
	})
	if err != nil {
		t.Fatalf("marshal heartbeat: %v", err)
	}
	host := client.ObjectKey{Namespace: testOperatorNamespace, Name: registeredName}
	t.Cleanup(func() {
		remove(t, k8s, &runtimev1alpha1.Host{ObjectMeta: metav1.ObjectMeta{Name: host.Name, Namespace: host.Namespace}})
	})
	// The cache sync does not wait for the heartbeat subscription, and a
	// heartbeat sent before it exists is dropped, so publish on every poll.
	eventually(t, "the heartbeat's Host", func() bool {
		if err := nc.Publish("runtime.operator.heartbeat.registered-id", heartbeat); err != nil {
			t.Fatalf("publish heartbeat: %v", err)
		}
		return get(t, k8s, host, &runtimev1alpha1.Host{})
	})

	deployment := &runtimev1alpha1.WorkloadDeployment{
		ObjectMeta: metav1.ObjectMeta{Name: registeredName, Namespace: testTenantNamespace},
		Spec: runtimev1alpha1.WorkloadDeploymentSpec{WorkloadReplicaSetSpec: runtimev1alpha1.WorkloadReplicaSetSpec{
			Replicas: new(int32(1)), Template: workloadTemplate(),
		}},
	}
	create(t, k8s, deployment)
	var replicaSet *runtimev1alpha1.WorkloadReplicaSet
	eventually(t, "the WorkloadDeployment's ReplicaSet", func() bool {
		var replicaSets runtimev1alpha1.WorkloadReplicaSetList
		if err := k8s.List(context.Background(), &replicaSets, client.InNamespace(testTenantNamespace)); err != nil {
			t.Fatalf("list replicasets: %v", err)
		}
		replicaSet = controlledBy(replicaSets.Items, deployment)
		return replicaSet != nil
	})
	eventually(t, "the ReplicaSet's Workload", func() bool {
		var workloads runtimev1alpha1.WorkloadList
		if err := k8s.List(context.Background(), &workloads, client.InNamespace(testTenantNamespace)); err != nil {
			t.Fatalf("list workloads: %v", err)
		}
		return controlledBy(workloads.Items, replicaSet) != nil
	})

	daemonSet := &runtimev1alpha1.WorkloadDaemonSet{
		ObjectMeta: metav1.ObjectMeta{Name: registeredName, Namespace: testTenantNamespace},
		Spec:       runtimev1alpha1.WorkloadDaemonSetSpec{Template: workloadTemplate()},
	}
	create(t, k8s, daemonSet)
	eventually(t, "the WorkloadDaemonSet's status", func() bool {
		var current runtimev1alpha1.WorkloadDaemonSet
		return get(t, k8s, client.ObjectKeyFromObject(daemonSet), &current) && len(current.Status.Conditions) > 0
	})
}

func TestNewEmbeddedOperatorCanLeaveArtifactsToAnotherController(t *testing.T) {
	cfg, k8s := sharedEnvtest(t)
	s := startNats(t)
	mgr := newManager(t, cfg, logr.Discard())
	operator := operatorConfig(s.ClientURL())
	operator.DisableArtifactController = true
	newOperator(t, mgr, operator)
	startManager(t, mgr)

	artifact := newArtifact("external")
	create(t, k8s, artifact)
	// A Pod created after the Artifact and reconciled shows the manager is
	// processing events; give the Artifact as long again before checking it.
	start := time.Now()
	pod := newHostPod("external")
	create(t, k8s, pod)
	eventually(t, "the host Pod's finalizer", func() bool { return podHasFinalizer(t, k8s, pod) })
	time.Sleep(time.Since(start) + time.Second)
	if artifactPublished(t, k8s, artifact) {
		t.Error("the Artifact was reconciled with the artifact controller disabled")
	}
}

// TestNewEmbeddedOperatorWaitsForMissingDaemonSetCRD covers an upgrade that
// skipped the chart's CRDs: everything else runs, and the manager exits once
// the CRD appears so the restarted pod registers its controller.
func TestNewEmbeddedOperatorWaitsForMissingDaemonSetCRD(t *testing.T) {
	cfg, k8s := sharedEnvtest(t)
	s := startNats(t)
	crds := envtest.CRDInstallOptions{Paths: []string{daemonSetCRDPath}, ErrorIfPathMissing: true}
	if err := envtest.UninstallCRDs(cfg, crds); err != nil {
		t.Fatalf("uninstall WorkloadDaemonSet CRD: %v", err)
	}
	t.Cleanup(func() {
		if _, err := envtest.InstallCRDs(cfg, crds); err != nil {
			t.Errorf("reinstall WorkloadDaemonSet CRD: %v", err)
		}
	})
	watch := &crdWatch{gv: runtimev1alpha1.GroupVersion, kind: "WorkloadDaemonSet"}
	var err error
	if watch.discovery, err = discovery.NewDiscoveryClientForConfig(cfg); err != nil {
		t.Fatalf("discovery client: %v", err)
	}
	eventually(t, "discovery to drop WorkloadDaemonSet", func() bool {
		installed, err := watch.installed()
		return err == nil && !installed
	})
	logs := &logRecorder{}
	census := &faultyManager{Manager: newManager(t, cfg, logs.logger()), watchInterval: 100 * time.Millisecond}
	newOperator(t, census, operatorConfig(s.ClientURL()))
	if got := logs.matching("WorkloadDaemonSet CRD is not installed"); len(got) != 1 {
		t.Errorf("missing-CRD log = %v, want one line", got)
	}
	// The watch is the last runnable added; without it nothing would restart
	// the pod once the CRD arrives.
	refused := &faultyManager{Manager: newManager(t, cfg, logr.Discard()), failAdd: census.adds}
	op, err := NewEmbeddedOperator(context.Background(), refused, operatorConfig(s.ClientURL()))
	if !errors.Is(err, errInjected) {
		t.Fatalf("NewEmbeddedOperator() with the CRD watch refused = %v, %v; want the injected failure", op, err)
	}
	mgr := census.Manager

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- mgr.Start(ctx) }()
	artifact := newArtifact("without-daemonsets")
	create(t, k8s, artifact)
	eventually(t, "the Artifact to be published", func() bool { return artifactPublished(t, k8s, artifact) })
	select {
	case err := <-done:
		t.Fatalf("manager stopped before the CRD was installed: %v", err)
	default:
	}

	if _, err := envtest.InstallCRDs(cfg, crds); err != nil {
		t.Fatalf("install WorkloadDaemonSet CRD: %v", err)
	}
	select {
	case err := <-done:
		if err == nil || !strings.Contains(err.Error(), "WorkloadDaemonSet CRD is now installed") {
			t.Fatalf("manager stopped with %v, want the CRD restart error", err)
		}
	case <-time.After(30 * time.Second):
		t.Fatal("manager kept running after the CRD was installed")
	}
}

// failingDiscovery fails the runtime API group's discovery once armed, on
// every transport it wraps.
type failingDiscovery struct{ armed atomic.Bool }

func (f *failingDiscovery) wrap(next http.RoundTripper) http.RoundTripper {
	return roundTripperFunc(func(req *http.Request) (*http.Response, error) {
		if f.armed.Load() && req.URL.Path == "/apis/"+runtimev1alpha1.GroupVersion.String() {
			return &http.Response{
				StatusCode: http.StatusServiceUnavailable,
				Header:     http.Header{"Content-Type": []string{"text/plain"}},
				Body:       io.NopCloser(strings.NewReader("discovery unavailable")),
				Request:    req,
			}, nil
		}
		return next.RoundTrip(req)
	})
}

type roundTripperFunc func(*http.Request) (*http.Response, error)

func (f roundTripperFunc) RoundTrip(req *http.Request) (*http.Response, error) { return f(req) }

// A discovery error is not a missing CRD: the operator must not start with the
// WorkloadDaemonSet controller off because the API server stumbled.
func TestNewEmbeddedOperatorRefusesWhenDiscoveryFails(t *testing.T) {
	cfg, _ := sharedEnvtest(t)
	s := startNats(t)
	faulty := &failingDiscovery{}
	wrapped := rest.CopyConfig(cfg)
	wrapped.WrapTransport = faulty.wrap
	mgr := newManager(t, wrapped, logr.Discard())
	// Field indexes resolve kinds through the manager's REST mapper; load the
	// group into it first so only the CRD check sees the failure.
	workloadKind := runtimev1alpha1.GroupVersion.WithKind("Workload").GroupKind()
	if _, err := mgr.GetRESTMapper().RESTMapping(workloadKind); err != nil {
		t.Fatalf("warm REST mapper: %v", err)
	}
	faulty.armed.Store(true)

	op, err := NewEmbeddedOperator(context.Background(), mgr, operatorConfig(s.ClientURL()))
	if !apierrors.IsServiceUnavailable(err) {
		t.Fatalf("NewEmbeddedOperator() = %v, %v; want the discovery error", op, err)
	}
	eventually(t, "the NATS connection to close", func() bool { return s.NumClients() == 0 })
}

// faultyManager fails the nth field index or runnable the operator registers,
// and hands out config in place of the manager's own when it is set. A
// nonzero watchInterval sets how often an added crdWatch polls.
type faultyManager struct {
	manager.Manager
	failIndex, failAdd int
	indexes, adds      int
	config             *rest.Config
	watchInterval      time.Duration
}

func (m *faultyManager) GetConfig() *rest.Config {
	if m.config != nil {
		return m.config
	}
	return m.Manager.GetConfig()
}

var errInjected = errors.New("injected registration failure")

func (m *faultyManager) GetFieldIndexer() client.FieldIndexer {
	return faultyIndexer{m}
}

func (m *faultyManager) Add(runnable manager.Runnable) error {
	if m.adds++; m.adds == m.failAdd {
		return errInjected
	}
	if watch, ok := runnable.(*crdWatch); ok && m.watchInterval > 0 {
		watch.interval = m.watchInterval
	}
	return m.Manager.Add(runnable)
}

type faultyIndexer struct{ m *faultyManager }

func (i faultyIndexer) IndexField(
	ctx context.Context, obj client.Object, field string, extract client.IndexerFunc,
) error {
	if i.m.indexes++; i.m.indexes == i.m.failIndex {
		return errInjected
	}
	return i.m.Manager.GetFieldIndexer().IndexField(ctx, obj, field, extract)
}

// Every registration step's failure reaches the caller, and the operator does
// not keep the NATS connection it opened for itself.
func TestNewEmbeddedOperatorReturnsRegistrationFailures(t *testing.T) {
	cfg, _ := sharedEnvtest(t)
	s := startNats(t)
	census := &faultyManager{Manager: newManager(t, cfg, logr.Discard())}
	newOperator(t, census, operatorConfig(s.ClientURL()))
	if census.indexes == 0 || census.adds == 0 {
		t.Fatalf("registered %d indexes and %d runnables, want some of each", census.indexes, census.adds)
	}

	var cases []faultyManager
	for n := 1; n <= census.indexes; n++ {
		cases = append(cases, faultyManager{failIndex: n})
	}
	for n := 1; n <= census.adds; n++ {
		cases = append(cases, faultyManager{failAdd: n})
	}
	unreadableCA := rest.CopyConfig(cfg)
	unreadableCA.CAData = []byte("not a certificate")
	cases = append(cases, faultyManager{config: unreadableCA})
	for _, tt := range cases {
		t.Run(fmt.Sprintf("index %d runnable %d config %t", tt.failIndex, tt.failAdd, tt.config != nil), func(t *testing.T) {
			before := s.NumClients()
			mgr := &faultyManager{
				Manager:   newManager(t, cfg, logr.Discard()),
				failIndex: tt.failIndex, failAdd: tt.failAdd, config: tt.config,
			}
			op, err := NewEmbeddedOperator(context.Background(), mgr, operatorConfig(s.ClientURL()))
			if tt.config == nil && !errors.Is(err, errInjected) {
				t.Fatalf("NewEmbeddedOperator() = %v, %v; want the injected failure", op, err)
			}
			if tt.config != nil && (err == nil || !strings.Contains(err.Error(), "root certificates")) {
				t.Fatalf("NewEmbeddedOperator() = %v, %v; want the discovery client's CA error", op, err)
			}
			eventually(t, "the NATS connection to close", func() bool { return s.NumClients() == before })
		})
	}
}
