package e2e

import (
	"fmt"
	"os/exec"
	"strings"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

// End-to-end coverage for host component plugins: a WebAssembly component loaded
// as a host capability provider (`wash host --host-plugin`, surfaced by the
// chart as `runtime.hostGroups[].hostPlugins`). It proves the whole path in a
// real cluster:
//
//   - chart: a hostGroup `hostPlugins` entry renders to a `--host-plugin` arg;
//   - host: `wash host` fetches the plugin image, builds it into its own
//     supervised store, and registers it before serving workloads;
//   - bind: a workload that imports the plugin's bespoke `acme:kv/store`
//     interface resolves against the component plugin (not a Rust plugin), and
//     the call hops across the store boundary; and
//   - roundtrip: a value written via `/set` reads back via `/get`, i.e. it
//     survived in the plugin's persistent store between two separate requests.
//
// The store-boundary mechanics (streams/futures/resources, per-caller
// partitioning, restart-on-trap) are covered in-process by
// crates/wash-runtime/tests/integration_component_host_plugin.rs; this spec
// proves the CLI/chart loading path and cross-store binding wire up in a real
// cluster.
//
// The plugin (kv-plugin, exporting acme:kv/store) and its caller
// (kv-plugin-caller, importing it and exposing it over HTTP) are P3 fixtures
// built and served from the in-cluster registry (make e2e-images), like every
// other e2e fixture. Unlike the workload-pull specs, a plugin is loaded at host
// STARTUP — so it can't be baked into the hostgroup the suite installs before
// the registry is serving. BeforeAll rolls the default hostgroup with the plugin
// via `helm upgrade` (the registry is up by then) and AfterAll restores a
// plugin-free host, keeping the spec independent of ordering.
//
// The host-component-plugins feature is not in release builds, so — like the
// implements spec — this runs only when the registry flow is on AND the fixture
// host is an all-features build (the all-features leg); it self-skips otherwise.
// Excluded from `make test`; runs in the dedicated `make test-e2e` job.
var _ = Describe("Host Component Plugin", Ordered, func() {
	const (
		pluginID     = "acme-kv"
		workloadName = "kv-plugin-caller"
		// Gateway routes by Host header to the component's HTTP export; matches
		// the `host` config on the workload's wasi:http interface below.
		workloadHost = "kv-plugin-caller.localhost.direct"
	)

	var pluginImage, callerImage string

	BeforeAll(func() {
		// Host component plugins need the host-component-plugins feature (not in
		// release builds) AND the in-cluster registry to fetch from — the same
		// gating as the implements spec. Off elsewhere, so self-skip.
		if !inClusterRegistry || !defaultHostAllFeatures {
			Skip("skipping host component plugin e2e (needs the in-cluster " +
				"registry and an all-features fixture host)")
		}
		pluginImage = registryRef("kv-plugin")
		callerImage = registryRef("kv-plugin-caller")

		// Load the plugin onto the default hostgroup by re-rendering its
		// Deployment with a `--host-plugin` arg and rolling the pod, so the host
		// fetches and registers the plugin at startup. AfterAll restores a
		// plugin-free host so this stays independent of spec ordering.
		// buildBaseHelmSets already makes the default hostgroup insecure (it
		// pulls fixtures from the plain-HTTP in-cluster registry), so the same
		// path fetches the plugin.
		By("configuring the default hostGroup with a host component plugin")
		sets := append(buildBaseHelmSets(),
			"runtime.hostGroups[0].hostPlugins[0].id="+pluginID,
			fmt.Sprintf("runtime.hostGroups[0].hostPlugins[0].image=%s", pluginImage),
		)
		Expect(helmUpgradeWait(sets)).To(Succeed(),
			"failed to load the host component plugin onto the default hostGroup")

		By("waiting for the hostgroup to roll out with the plugin")
		cmd := exec.Command("kubectl", "rollout", "status",
			"-n", namespace, "deployment/hostgroup-default", "--timeout=3m")
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(),
			"hostgroup did not become ready with the plugin loaded — "+
				"a host that cannot fetch/build a configured plugin fails to start")

		By("waiting for a Host CR to be registered")
		Eventually(func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "hosts.runtime.wasmcloud.dev",
				"-n", namespace, "-o", "jsonpath={.items}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(output).NotTo(Equal("[]"), "no Host CR registered yet")
		}).WithTimeout(2 * time.Minute).Should(Succeed())
	})

	AfterEach(func() {
		if !CurrentSpecReport().Failed() {
			return
		}
		dump := func(label string, args ...string) {
			out, err := utils.Run(exec.Command("kubectl", args...))
			if err == nil {
				_, _ = fmt.Fprintf(GinkgoWriter, "=== %s ===\n%s\n", label, out)
			} else {
				_, _ = fmt.Fprintf(GinkgoWriter, "=== %s (FAILED: %s) ===\n", label, err)
			}
		}
		// The hostgroup logs are the most direct evidence of where it broke:
		// plugin image pull, plugin build (exports no capability), or the
		// workload's cross-store bind.
		dump("Pods", "get", "pods", "-n", namespace, "-o", "wide")
		dump("Events", "get", "events", "-n", namespace, "--sort-by=.lastTimestamp")
		dump("Hostgroup logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=600", "--prefix=true")
		dump("WorkloadDeployment", "get", "workloaddeployment", workloadName,
			"-n", namespace, "-o", "yaml")
		dump("Workload CRs", "get", "workloads.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
	})

	AfterAll(func() {
		if pluginImage == "" {
			return
		}
		_ = exec.Command("kubectl", "delete", "workloaddeployment", workloadName,
			"-n", namespace, "--ignore-not-found=true").Run()
		// Restore a plugin-free host so later specs see a clean default hostgroup
		// regardless of ordering (dropping the hostPlugins `--set` removes the arg).
		By("restoring a plugin-free hostGroup")
		if err := helmUpgradeWait(buildBaseHelmSets()); err != nil {
			_, _ = fmt.Fprintf(GinkgoWriter, "WARNING: failed to restore hostGroup: %s\n", err)
			return
		}
		_, _ = utils.Run(exec.Command("kubectl", "rollout", "status",
			"-n", namespace, "deployment/hostgroup-default", "--timeout=3m"))
	})

	It("serves a workload's imported capability from the component plugin", func() {
		By("deploying a workload that imports acme:kv/store")
		// The host satisfies the `acme:kv/store` import with the registered
		// component plugin; the wasi:http interface declares this component's real
		// P3 `handler@0.3.0` export, which the host serves and the runtime-gateway
		// now routes by Host header (the gateway was taught to register routes for
		// p3 `handler`, not just p2 `incoming-handler`).
		manifest := fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: 1
  template:
    spec:
      # Pin to the default hostgroup — the one carrying the plugin, and insecure
      # so it can pull the caller from the in-cluster (plain-HTTP) registry. The
      # registry hostgroup stays on HTTPS and has no plugin.
      hostSelector:
        hostgroup: default
      hostInterfaces:
        - namespace: wasi
          package: http
          version: "0.3.0"
          interfaces:
            - handler
          config:
            host: %s
        - namespace: acme
          package: kv
          version: "0.1.0"
          interfaces:
            - store
      components:
        - name: %s
          image: %s
`, workloadName, namespace, workloadHost, workloadName, callerImage)

		cmd := exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "failed to apply the caller WorkloadDeployment")

		By("waiting for the WorkloadDeployment to become Ready")
		// Ready proves the host bound the component's acme:kv import to the
		// plugin — without a matching provider the workload never instantiates.
		Eventually(func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "workloaddeployment", workloadName,
				"-n", namespace,
				"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(output).To(Equal("True"))
		}).WithTimeout(3 * time.Minute).Should(Succeed())

		By("writing a key through the plugin (/set)")
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("curl", "-s", "-o", "/dev/null",
				"-w", "%{http_code}", "-H", "Host: "+workloadHost,
				"http://localhost:80/set?key=greeting&value=hello"))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(out).To(Equal("200"), "/set should route through the plugin")
		}).WithTimeout(2 * time.Minute).Should(Succeed())

		By("reading it back (/get) — the value survived in the plugin's store")
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("curl", "-s", "-H", "Host: "+workloadHost,
				"http://localhost:80/get?key=greeting"))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(strings.TrimSpace(out)).To(Equal("hello"),
				"the value must round-trip through the component plugin's persistent store")
		}).WithTimeout(1 * time.Minute).Should(Succeed())
	})
})

// helmUpgradeWait runs `helm upgrade --install --reset-values` for the
// operator-e2e release with the given `--set` values and waits for it to
// converge. --reset-values renders purely from buildBaseHelmSets (plus what's
// passed), discarding any prior spec's upgrade --sets — so loading and then
// dropping the `--host-plugin` arg on the default hostGroup is independent of
// test ordering.
func helmUpgradeWait(sets []string) error {
	helmArgs := make([]string, 0, 6+2*len(sets)+3)
	helmArgs = append(helmArgs, "upgrade", "--install", "--reset-values", "-n", namespace)
	for _, s := range sets {
		helmArgs = append(helmArgs, "--set", s)
	}
	helmArgs = append(helmArgs, "--wait", "--timeout=5m", "operator-e2e", helmChartPath)
	_, err := utils.Run(exec.Command("helm", helmArgs...))
	return err
}
