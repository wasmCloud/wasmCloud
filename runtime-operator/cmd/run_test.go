package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/nats-io/nats-server/v2/server"
	"github.com/nats-io/nats.go"
	"google.golang.org/protobuf/encoding/protojson"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/rest"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimectrl "go.wasmcloud.dev/runtime-operator/v2/internal/controller/runtime"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
	"go.wasmcloud.dev/runtime-operator/v2/pkg/wasmbus"
)

const (
	operatorNamespace  = "operator-system"
	watchedNamespace   = "tenant-watched"
	unwatchedNamespace = "tenant-unwatched"
	hostPodNamespace   = "host-pods"
	podFinalizer       = "runtime.wasmcloud.dev/pod-host-finalizer"
	firstToken         = "first-token"
	rotatedToken       = "rotated-token"
	// authGrace outlasts the operator's 1-2s reconnect cadence, so a run of
	// refusals never goes stale between attempts.
	authGrace = 3 * time.Second
)

// runWith runs the operator with a fresh flag set, as main does with the
// process's own.
func runWith(ctx context.Context, args []string, getConfig func() (*rest.Config, error)) error {
	fs := flag.NewFlagSet("runtime-operator", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	return run(ctx, fs, args, getConfig)
}

// offlineConfig points at nothing; a manager can be built from it without
// contacting a server.
func offlineConfig() (*rest.Config, error) {
	return &rest.Config{Host: "http://127.0.0.1:1"}, nil
}

func TestRunRejectsConfigurationBeforeStarting(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "missing.pem")
	noListeners := []string{"--metrics-bind-address=0", "--health-probe-bind-address=0"}
	for _, tt := range []struct {
		name      string
		namespace string
		args      []string
		getConfig func() (*rest.Config, error)
		// cluster runs against envtest: building the manager's cache reads
		// REST mappings from the API server.
		cluster bool
		want    string
		wantIs  []error
	}{
		{
			name: "unknown flag", namespace: operatorNamespace,
			args: []string{"--no-such-flag"}, want: "flag provided but not defined",
		},
		{
			name: "operator namespace unset",
			args: noListeners, want: "OPERATOR_NAMESPACE is unset",
		},
		{
			name: "NATS user without a password", namespace: operatorNamespace,
			args: append([]string{"--nats-user=operator"}, noListeners...),
			want: "invalid NATS client configuration: -nats-user requires -nats-password-file",
		},
		{
			name: "kubeconfig unavailable", namespace: operatorNamespace, args: noListeners,
			getConfig: func() (*rest.Config, error) { return nil, errors.New("no kubeconfig") },
			want:      "unable to get kubeconfig: no kubeconfig",
		},
		{
			name: "leader election outside a cluster", namespace: operatorNamespace,
			args:    append([]string{"--leader-elect"}, noListeners...),
			cluster: true,
			want:    "unable to start manager: unable to find leader election namespace",
		},
		{
			name: "NATS TLS files missing", namespace: operatorNamespace,
			args: append([]string{
				"--nats-url=nats://127.0.0.1:1",
				"--nats-ca=" + missing,
				"--nats-client-cert=" + missing,
				"--nats-client-key=" + missing,
				"--nats-tls-first",
			}, noListeners...),
			cluster: true,
			want:    "unable to create runtime operator: transport error: nats: error loading or parsing rootCA file",
			wantIs:  []error{wasmbus.ErrTransport},
		},
	} {
		t.Run(tt.name, func(t *testing.T) {
			t.Setenv("OPERATOR_NAMESPACE", tt.namespace)
			getConfig := tt.getConfig
			switch {
			case tt.cluster:
				cfg, _ := sharedEnvtest(t)
				getConfig = func() (*rest.Config, error) { return cfg, nil }
			case getConfig == nil:
				getConfig = offlineConfig
			}
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			defer cancel()

			err := runWith(ctx, tt.args, getConfig)
			if err == nil || !strings.Contains(err.Error(), tt.want) {
				t.Fatalf("run() = %v, want an error containing %q", err, tt.want)
			}
			for _, target := range tt.wantIs {
				if !errors.Is(err, target) {
					t.Errorf("run() = %v, want it to wrap %v", err, target)
				}
			}
		})
	}
}

// TestRunServesUntilCancelled drives the operator binary's whole lifecycle
// against envtest and an embedded NATS server. Controller names are unique per
// process, so this is the only test here that gets past controller setup.
func TestRunServesUntilCancelled(t *testing.T) {
	cfg, k8s := sharedEnvtest(t)
	for _, ns := range []string{operatorNamespace, watchedNamespace, unwatchedNamespace, hostPodNamespace} {
		if err := k8s.Create(context.Background(), &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: ns}}); err != nil {
			t.Fatalf("create namespace %s: %v", ns, err)
		}
	}

	tokenFile := filepath.Join(t.TempDir(), "token")
	writeToken(t, tokenFile, firstToken)
	natsServer := startNats(t, server.RANDOM_PORT, firstToken)
	natsPort := natsServer.Addr().(*net.TCPAddr).Port
	probeAddr := freeAddr(t)

	t.Setenv("OPERATOR_NAMESPACE", operatorNamespace)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() {
		done <- runWith(ctx, []string{
			"--nats-url=" + natsServer.ClientURL(),
			"--nats-token-file=" + tokenFile,
			"--nats-auth-failure-grace=" + authGrace.String(),
			"--health-probe-bind-address=" + probeAddr,
			"--metrics-bind-address=127.0.0.1:0",
			"--metrics-secure",
			"--json-log",
			"--watch-namespaces=" + watchedNamespace,
			"--host-namespaces=" + hostPodNamespace,
		}, func() (*rest.Config, error) { return cfg, nil })
	}()

	probeClient := &http.Client{Timeout: 5 * time.Second}
	probe := func(path string) (int, string) {
		resp, err := probeClient.Get("http://" + probeAddr + path)
		if err != nil {
			return 0, err.Error()
		}
		defer func() { _ = resp.Body.Close() }()
		body, _ := io.ReadAll(resp.Body)
		return resp.StatusCode, string(body)
	}
	expectProbe := func(path string, status int, bodyContains string) {
		t.Helper()
		eventually(t, done, fmt.Sprintf("%s to answer %d %q", path, status, bodyContains), func() bool {
			got, body := probe(path)
			return got == status && strings.Contains(body, bodyContains)
		})
	}
	for _, path := range []string{"/healthz", "/healthz/nats", "/readyz"} {
		expectProbe(path, http.StatusOK, "")
	}

	// The Artifact controller only caches -watch-namespaces.
	unwatched := createArtifact(t, k8s, unwatchedNamespace)
	watched := createArtifact(t, k8s, watchedNamespace)
	eventually(t, done, "the watched Artifact to be published", func() bool {
		return artifactURL(t, k8s, watched) != ""
	})
	if url := artifactURL(t, k8s, unwatched); url != "" {
		t.Errorf("Artifact outside -watch-namespaces was reconciled: artifactUrl %q", url)
	}

	// Host Pods are managed in the operator's namespace and -host-namespaces,
	// and nowhere else.
	strayPod := createHostPod(t, k8s, watchedNamespace)
	listedPod := createHostPod(t, k8s, hostPodNamespace)
	ownPod := createHostPod(t, k8s, operatorNamespace)
	for _, pod := range []client.ObjectKey{listedPod, ownPod} {
		eventually(t, done, "host Pod "+pod.String()+" to carry its finalizer", func() bool {
			return hasPodFinalizer(t, k8s, pod)
		})
	}
	if hasPodFinalizer(t, k8s, strayPod) {
		t.Errorf("host Pod %s outside -host-namespaces got the finalizer", strayPod)
	}

	publisher := connectNats(t, natsServer.ClientURL(), firstToken)
	expectHeartbeatRegisters(t, k8s, publisher, done, "before-outage", "10.0.0.1")

	// An outage is not a credential problem: liveness holds through one longer
	// than the grace, and the operator hears heartbeats again once it is back.
	natsServer.Shutdown()
	natsServer.WaitForShutdown()
	for until := time.Now().Add(authGrace + time.Second); time.Now().Before(until); time.Sleep(100 * time.Millisecond) {
		if status, body := probe("/healthz"); status != http.StatusOK {
			t.Fatalf("/healthz during a NATS outage = %d %s, want 200", status, body)
		}
	}
	natsServer = startNats(t, natsPort, firstToken)
	publisher = connectNats(t, natsServer.ClientURL(), firstToken)
	expectHeartbeatRegisters(t, k8s, publisher, done, "after-outage", "10.0.0.2")

	// A refused credential fails liveness once it outlasts the grace, and the
	// operator reconnects when the mounted token catches up.
	if err := natsServer.ReloadOptions(natsOptions(natsPort, rotatedToken)); err != nil {
		t.Fatalf("rotate NATS token: %v", err)
	}
	expectProbe("/healthz/nats", http.StatusInternalServerError, "")
	expectProbe("/healthz", http.StatusInternalServerError, "nats failed")
	writeToken(t, tokenFile, rotatedToken)
	publisher = connectNats(t, natsServer.ClientURL(), rotatedToken)
	expectHeartbeatRegisters(t, k8s, publisher, done, "after-rotation", "10.0.0.3")
	expectProbe("/healthz", http.StatusOK, "")

	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("run() after cancel = %v, want nil", err)
		}
	case <-time.After(30 * time.Second):
		t.Fatal("run() did not return after its context was cancelled")
	}
	if status, _ := probe("/healthz"); status != 0 {
		t.Errorf("/healthz answered %d after shutdown", status)
	}
}

var shared struct {
	once sync.Once
	env  *envtest.Environment
	cfg  *rest.Config
	k8s  client.Client
	err  error
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

// sharedEnvtest starts one API server for the package on first use.
func sharedEnvtest(t *testing.T) (*rest.Config, client.Client) {
	t.Helper()
	if os.Getenv("KUBEBUILDER_ASSETS") == "" {
		t.Skip("KUBEBUILDER_ASSETS not set; run via `make test` for envtest")
	}
	shared.once.Do(func() {
		shared.env = &envtest.Environment{
			CRDDirectoryPaths:     []string{filepath.Join("..", "config", "crd", "bases")},
			ErrorIfCRDPathMissing: true,
		}
		if shared.cfg, shared.err = shared.env.Start(); shared.err != nil {
			return
		}
		shared.k8s, shared.err = client.New(shared.cfg, client.Options{Scheme: scheme})
	})
	if shared.err != nil {
		t.Fatalf("start envtest: %v", shared.err)
	}
	return shared.cfg, shared.k8s
}

func natsOptions(port int, token string) *server.Options {
	return &server.Options{Host: "127.0.0.1", Port: port, Authorization: token, NoLog: true, NoSigs: true}
}

// startNats starts an embedded NATS server on port that accepts token.
func startNats(t *testing.T, port int, token string) *server.Server {
	t.Helper()
	s, err := server.NewServer(natsOptions(port, token))
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

func connectNats(t *testing.T, url, token string) *nats.Conn {
	t.Helper()
	nc, err := nats.Connect(url, nats.Token(token))
	if err != nil {
		t.Fatalf("connect to nats: %v", err)
	}
	t.Cleanup(nc.Close)
	return nc
}

func writeToken(t *testing.T, path, token string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(token), 0o600); err != nil {
		t.Fatalf("write token: %v", err)
	}
}

// freeAddr returns a loopback address with a port nothing is listening on.
func freeAddr(t *testing.T) string {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("reserve a port: %v", err)
	}
	addr := l.Addr().String()
	if err := l.Close(); err != nil {
		t.Fatalf("release port: %v", err)
	}
	return addr
}

// eventually polls cond, failing early if run returns first.
func eventually(t *testing.T, done <-chan error, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(30 * time.Second)
	for !cond() {
		select {
		case err := <-done:
			t.Fatalf("run() returned %v while waiting for %s", err, what)
		case <-time.After(100 * time.Millisecond):
		}
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
	}
}

func createArtifact(t *testing.T, k8s client.Client, namespace string) client.ObjectKey {
	t.Helper()
	artifact := &runtimev1alpha1.Artifact{
		ObjectMeta: metav1.ObjectMeta{Name: "artifact", Namespace: namespace},
		Spec:       runtimev1alpha1.ArtifactSpec{Image: "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0"},
	}
	if err := k8s.Create(context.Background(), artifact); err != nil {
		t.Fatalf("create artifact in %s: %v", namespace, err)
	}
	return client.ObjectKeyFromObject(artifact)
}

func artifactURL(t *testing.T, k8s client.Client, key client.ObjectKey) string {
	t.Helper()
	var artifact runtimev1alpha1.Artifact
	if err := k8s.Get(context.Background(), key, &artifact); err != nil {
		t.Fatalf("get artifact %s: %v", key, err)
	}
	return artifact.Status.ArtifactURL
}

func createHostPod(t *testing.T, k8s client.Client, namespace string) client.ObjectKey {
	t.Helper()
	pod := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name: "host", Namespace: namespace,
			Labels: map[string]string{runtimectrl.HostPodLabel: "default"},
		},
		Spec: corev1.PodSpec{Containers: []corev1.Container{{Name: "host", Image: "wasmcloud/wash"}}},
	}
	if err := k8s.Create(context.Background(), pod); err != nil {
		t.Fatalf("create host pod in %s: %v", namespace, err)
	}
	key := client.ObjectKeyFromObject(pod)
	// envtest has no kubelet to honour the finalizer, so strip it to let the
	// namespace's Pods go with the environment.
	t.Cleanup(func() {
		var current corev1.Pod
		if err := k8s.Get(context.Background(), key, &current); err != nil {
			return
		}
		current.Finalizers = nil
		_ = k8s.Update(context.Background(), &current)
	})
	return key
}

func hasPodFinalizer(t *testing.T, k8s client.Client, key client.ObjectKey) bool {
	t.Helper()
	var pod corev1.Pod
	if err := k8s.Get(context.Background(), key, &pod); err != nil {
		t.Fatalf("get pod %s: %v", key, err)
	}
	for _, finalizer := range pod.Finalizers {
		if finalizer == podFinalizer {
			return true
		}
	}
	return false
}

// expectHeartbeatRegisters heartbeats as the host name until its Host appears
// in the operator's namespace. Each attempt republishes, since a heartbeat
// sent before the operator (re)subscribed is not delivered.
func expectHeartbeatRegisters(
	t *testing.T, k8s client.Client, nc *nats.Conn, done <-chan error, name, hostname string,
) {
	t.Helper()
	data, err := protojson.Marshal(&runtimev2.HostHeartbeat{Id: name + "-id", FriendlyName: name, Hostname: hostname})
	if err != nil {
		t.Fatalf("marshal heartbeat: %v", err)
	}
	key := types.NamespacedName{Namespace: operatorNamespace, Name: name}
	eventually(t, done, "Host "+name, func() bool {
		if err := nc.Publish("runtime.operator.heartbeat."+name, data); err != nil {
			t.Fatalf("publish heartbeat: %v", err)
		}
		err := k8s.Get(context.Background(), key, &runtimev1alpha1.Host{})
		if err != nil && !apierrors.IsNotFound(err) {
			t.Fatalf("get host %s: %v", key, err)
		}
		return err == nil
	})
}
