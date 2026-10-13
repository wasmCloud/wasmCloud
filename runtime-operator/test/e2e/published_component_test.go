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

// End-to-end coverage for a component published before the host that runs it
// was built: `oci-registry:0.1.0`, whose wit-bindgen 0.58 async runtime traps
// under wasmtime 48 unless the host reorders its cancel at load. In-process
// coverage is crates/wash-runtime/tests/integration_guest_compat.rs. Skips on
// the canary host, which is only as new as main.
var _ = Describe("Published Component", Ordered, func() {
	const (
		workloadName = "published-legacy-registry"
		// Gateway routes by Host header; matches the `host` config on the
		// workload's wasi:http interface below.
		workloadHost = "published-legacy-registry.localhost.direct"
		// ghcr.io/wasmcloud/components/oci-registry:0.1.0. By digest, so what
		// runs is the build that carries the legacy runtime whatever the tag
		// comes to point at.
		componentImage = "ghcr.io/wasmcloud/components/oci-registry" +
			"@sha256:0d02f54a06f19d52655a2663a419e06ea59e3aa0e20bf0e2ba95f611dd9f20cc"
		blob       = "published before the trap was enforced"
		blobDigest = "sha256:906b53bdc8191b17951d25d4e3cacbb81853a8e042a38ca27a1d888ebe08d63d"
	)

	var deployed bool

	BeforeAll(func() {
		if !buildRuntimeImage {
			Skip("skipping published component e2e (needs a host built from this tree)")
		}

		By("ensuring at least one hostgroup pod is running")
		cmd := exec.Command("kubectl", "scale", "deployment/hostgroup-default",
			"--replicas=1", "-n", namespace)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "Failed to scale hostgroup")

		cmd = exec.Command("kubectl", "rollout", "status",
			"-n", namespace, "deployment/hostgroup-default", "--timeout=2m")
		_, err = utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "hostgroup rollout did not complete")

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
		// The hostgroup logs say which it was: the pull from ghcr, the compile,
		// or a trap.
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
		if !deployed {
			return
		}
		_ = exec.Command("kubectl", "delete", "workloaddeployment", workloadName,
			"-n", namespace, "--ignore-not-found=true").Run()
	})

	It("serves a call that suspends, on a component built before the host", func() {
		By("deploying the published component by digest")
		manifest := fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: 1
  template:
    spec:
      # The default hostgroup is the one the gateway can reach: the registry
      # hostgroup serves TLS, and the gateway proxies in plaintext.
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
        - namespace: wasmcloud
          package: blobstore
          version: "0.1.0"
          interfaces:
            - blobstore
          config:
            backend: in-memory
      components:
        - name: oci-registry
          image: %s
`, workloadName, namespace, workloadHost, componentImage)

		cmd := exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "failed to apply the WorkloadDeployment")
		deployed = true

		By("waiting for the WorkloadDeployment to become Ready")
		Eventually(func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "workloaddeployment", workloadName,
				"-n", namespace,
				"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(output).To(Equal("True"))
		}).WithTimeout(3 * time.Minute).Should(Succeed())

		// `/v2/` is answered before the registry touches the blobstore, so it
		// returns without suspending. It says the route is up and nothing else,
		// which is what lets the push below be asserted once instead of retried
		// until it happens to pass.
		By("waiting for the gateway to route to the workload")
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("curl", "-s", "-o", "/dev/null",
				"-w", "%{http_code}", "-H", "Host: "+workloadHost, gatewayURL("/v2/")))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(out).To(Equal("200"))
		}).WithTimeout(2 * time.Minute).Should(Succeed())

		By("pushing a blob, which suspends the call on the blobstore")
		out, err := utils.Run(exec.Command("curl", "-s", "-o", "/dev/null",
			"-w", "%{http_code}", "-X", "POST",
			"-H", "Host: "+workloadHost,
			"-H", "Content-Type: application/octet-stream",
			"--data-binary", blob,
			gatewayURL("/v2/compat/blobs/uploads/?digest="+blobDigest)))
		Expect(err).NotTo(HaveOccurred())
		Expect(strings.TrimSpace(out)).To(Equal("201"),
			"a 500 here is the component trapping when its suspended call is "+
				"woken: the host compiled it without reordering its cancel")

		By("pulling the blob back")
		out, err = utils.Run(exec.Command("curl", "-s", "-H", "Host: "+workloadHost,
			gatewayURL("/v2/compat/blobs/"+blobDigest)))
		Expect(err).NotTo(HaveOccurred())
		Expect(strings.TrimSpace(out)).To(Equal(blob))
	})
})
