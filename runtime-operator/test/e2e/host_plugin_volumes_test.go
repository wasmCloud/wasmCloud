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

// End-to-end coverage for host component plugin volumes declared through the
// chart: a `plugins` entry's `volumes` (any Pod volume source) and
// `volumeMounts`. It proves the path in a real cluster:
//
//   - chart: each volume becomes a Pod volume mounted into the host container,
//     read-only when every mount of it is, and the entry renders into the
//     `wash host` config file with that directory as the volume's `hostPath`;
//   - host: `wash host` opens the directories at plugin load and preopens them
//     in the plugin's store at each `mountPath`; and
//   - plugin: a workload calling the plugin reads a ConfigMap key through a
//     read-only mount, and round-trips a write through a writable emptyDir.
//     The same emptyDir is also mounted read-only at a second path, where the
//     write is refused. The volume itself is writable there, so only the
//     plugin mount's `readOnly` can refuse it.
//
// The runtime's own volume rules (reserved paths, escape attempts, a plugin
// with no volumes) are covered in-process by
// crates/wash-runtime/tests/integration_plugin_volumes.rs.
//
// volume-plugin (exporting acme:volume/files) and volume-plugin-caller
// (importing it and exposing it over HTTP) are P3 fixtures served from the
// in-cluster registry, as in the host component plugin spec. Gated the same
// way: the in-cluster registry and an all-features fixture host. Excluded from
// `make test`; runs in the dedicated `make test-e2e` job.
var _ = Describe("Host Component Plugin Volumes", Ordered, func() {
	const (
		pluginID      = "volume-plugin"
		workloadName  = "volume-plugin-caller"
		workloadHost  = "volume-plugin-caller.localhost.direct"
		configMapName = "plugin-volume-data"
		fileContents  = "hello from a configmap"
	)

	var pluginImage, callerImage string

	// get asks the caller to run one file operation through the plugin and
	// returns the plugin's outcome: "ok", "ok:<contents>" or "error...".
	get := func(path string) func(g Gomega) string {
		return func(g Gomega) string {
			out, err := utils.Run(exec.Command("curl", "-s", "-H", "Host: "+workloadHost,
				gatewayURL(path)))
			g.Expect(err).NotTo(HaveOccurred())
			return strings.TrimSpace(out)
		}
	}

	BeforeAll(func() {
		if !inClusterRegistry || !defaultHostAllFeatures {
			Skip("skipping host component plugin volumes e2e (needs the in-cluster " +
				"registry and an all-features fixture host)")
		}
		pluginImage = registryRef(pluginID)
		callerImage = registryRef(workloadName)

		By("creating the ConfigMap the plugin serves from")
		cmd := exec.Command("kubectl", "create", "configmap", configMapName,
			"-n", namespace, "--from-literal=hello.txt="+fileContents,
			"--dry-run=client", "-o", "yaml")
		manifest, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred())
		cmd = exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err = utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "failed to create the plugin's ConfigMap")

		By("configuring the default hostGroup with a plugin that mounts volumes")
		// The plugin is pulled at host startup with the registry credentials
		// from the host's environment, as in the host component plugin spec.
		sets := append(buildBaseHelmSets(),
			"runtime.hostGroups[0].plugins[0].id="+pluginID,
			"runtime.hostGroups[0].plugins[0].image="+pluginImage,
			"runtime.hostGroups[0].env[0].name=WASH_HOST_PLUGIN_REGISTRY_USER",
			"runtime.hostGroups[0].env[0].value="+registryUser,
			"runtime.hostGroups[0].env[1].name=WASH_HOST_PLUGIN_REGISTRY_PASSWORD",
			"runtime.hostGroups[0].env[1].value="+registryPassword,
		)
		volumes := fmt.Sprintf(`[{"name":"data","configMap":{"name":%q}},`+
			`{"name":"scratch","emptyDir":{}}]`, configMapName)
		mounts := `[{"name":"data","mountPath":"/data","readOnly":true},` +
			`{"name":"scratch","mountPath":"/scratch"},` +
			`{"name":"scratch","mountPath":"/scratch-ro","readOnly":true}]`
		Expect(helmUpgradeWait(sets,
			"--set-json", "runtime.hostGroups[0].plugins[0].volumes="+volumes,
			"--set-json", "runtime.hostGroups[0].plugins[0].volumeMounts="+mounts,
		)).To(Succeed(), "failed to load the volume plugin onto the default hostGroup")

		By("waiting for the hostgroup to roll out with the plugin")
		_, err = utils.Run(exec.Command("kubectl", "rollout", "status",
			"-n", namespace, "deployment/hostgroup-default", "--timeout=3m"))
		Expect(err).NotTo(HaveOccurred(),
			"hostgroup did not become ready: a host whose plugin volume cannot be "+
				"opened fails to start")

		By("deploying a workload that imports acme:volume/files")
		manifest = fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: 1
  template:
    spec:
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
          package: volume
          version: "0.1.0"
          interfaces:
            - files
      components:
        - name: %s
          image: %s
`, workloadName, namespace, workloadHost, workloadName, callerImage)
		cmd = exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err = utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "failed to apply the caller WorkloadDeployment")

		By("waiting for the WorkloadDeployment to become Ready")
		Eventually(func(g Gomega) {
			output, err := utils.Run(exec.Command("kubectl", "get", "workloaddeployment",
				workloadName, "-n", namespace,
				"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}"))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(output).To(Equal("True"))
		}).WithTimeout(3 * time.Minute).Should(Succeed())
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
		dump("Pods", "get", "pods", "-n", namespace, "-o", "wide")
		dump("Events", "get", "events", "-n", namespace, "--sort-by=.lastTimestamp")
		dump("Host config", "get", "configmap", "hostgroup-default-host-config",
			"-n", namespace, "-o", "yaml")
		dump("Hostgroup logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=600", "--prefix=true")
		dump("WorkloadDeployment", "get", "workloaddeployment", workloadName,
			"-n", namespace, "-o", "yaml")
	})

	AfterAll(func() {
		if pluginImage == "" {
			return
		}
		_ = exec.Command("kubectl", "delete", "workloaddeployment", workloadName,
			"-n", namespace, "--ignore-not-found=true").Run()
		By("restoring a plugin-free hostGroup")
		if err := helmUpgradeWait(buildBaseHelmSets()); err != nil {
			_, _ = fmt.Fprintf(GinkgoWriter, "WARNING: failed to restore hostGroup: %s\n", err)
		} else {
			_, _ = utils.Run(exec.Command("kubectl", "rollout", "status",
				"-n", namespace, "deployment/hostgroup-default", "--timeout=3m"))
		}
		_ = exec.Command("kubectl", "delete", "configmap", configMapName,
			"-n", namespace, "--ignore-not-found=true").Run()
	})

	It("mounts each plugin volume into the host container, read-only unless a mount writes", func() {
		output, err := utils.Run(exec.Command("kubectl", "get", "deployment",
			"hostgroup-default", "-n", namespace, "-o",
			`jsonpath={range .spec.template.spec.containers[0].volumeMounts[*]}`+
				`{.mountPath}={.readOnly}{"\n"}{end}`))
		Expect(err).NotTo(HaveOccurred())
		Expect(output).To(ContainSubstring(
			"/var/lib/wasmcloud/host-plugin-volumes/" + pluginID + "/data=true"))
		Expect(output).To(MatchRegexp(
			`(?m)^/var/lib/wasmcloud/host-plugin-volumes/` + pluginID + `/scratch=$`))
	})

	It("reads a ConfigMap key through a read-only mount", func() {
		Eventually(get("/read?path=/data/hello.txt")).
			WithTimeout(2 * time.Minute).
			Should(Equal("ok:" + fileContents))
	})

	It("round-trips a write through a writable emptyDir mount", func() {
		Eventually(get("/write?path=/scratch/out.txt&contents=written")).
			WithTimeout(1 * time.Minute).
			Should(Equal("ok"))
		Eventually(get("/read?path=/scratch/out.txt")).
			WithTimeout(1 * time.Minute).
			Should(Equal("ok:written"))
	})

	It("refuses a write through a read-only mount of a writable volume", func() {
		By("seeing the writable mount's file through the read-only mount")
		Eventually(get("/write?path=/scratch/shared.txt&contents=shared")).
			WithTimeout(1 * time.Minute).
			Should(Equal("ok"))
		Eventually(get("/read?path=/scratch-ro/shared.txt")).
			WithTimeout(1 * time.Minute).
			Should(Equal("ok:shared"))

		By("being refused a write there")
		Eventually(get("/write?path=/scratch-ro/new.txt&contents=x")).
			WithTimeout(1 * time.Minute).
			Should(HavePrefix("error"))
		Eventually(get("/read?path=/scratch/new.txt")).
			WithTimeout(1*time.Minute).
			Should(HavePrefix("error"), "a refused write must leave no file in the volume")
	})
})
