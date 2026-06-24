package e2e

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

// End-to-end coverage for `(implements ..)` named host interfaces: a single
// WorkloadDeployment that declares the *same* namespace:package (wasi:keyvalue)
// twice under distinct names (`team-a`, `team-b`), each routed to its own
// backend. Reaching Ready exercises the whole stack end-to-end:
//
//   - admission: the CEL XValidation rules on WorkloadSpec.hostInterfaces accept
//     two same-package entries because they carry distinct names (and still
//     reject true duplicates / a second unnamed entry);
//   - reconcile/dedup: EnsureHostInterface keeps the two named entries DISTINCT
//     on the derived Workload CR (different names never merge); and
//   - host: the multiplexed keyvalue plugin (registered in `wash host` under the
//     `wasm_component_model_implements` feature) binds the component's `team-a`
//     and `team-b` `(implements ..)` imports to separate backends — without it
//     the WorkloadDeployment can't instantiate and never reaches Ready.
//
// The byte-level isolation check (write `from-a` via team-a, `from-b` via
// team-b, read back `isolated`) is covered in-process by
// crates/wash-runtime/tests/integration_keyvalue_implements.rs; this spec proves
// the operator and host wire that routing up in a real cluster. The
// merge/canon-version logic has direct unit tests in
// api/runtime/v1alpha1/workload_types_test.go, and envtest (`make test`)
// validates the CEL rules against a live apiserver.
//
// To run this spec, set IMPLEMENTS_E2E_IMAGE to an OCI ref the cluster can pull
// for a component that exports wasi:http/incoming-handler and imports
// wasi:keyvalue/store twice under the labels `team-a` and `team-b` (build from
// crates/wash-runtime/tests/fixtures/keyvalue-implements). Excluded from
// `make test` (which skips ./test/e2e); runs in the dedicated `make test-e2e`
// job, which sets IMPLEMENTS_E2E_IMAGE, and skips when that's unset.
var _ = Describe("Implements Named Host Interfaces", Ordered, func() {
	const workloadName = "keyvalue-implements"

	var componentImage string

	BeforeAll(func() {
		componentImage = os.Getenv("IMPLEMENTS_E2E_IMAGE")
		if componentImage == "" {
			Skip("IMPLEMENTS_E2E_IMAGE not set; skipping implements e2e " +
				"(see runtime-operator/test/e2e/implements_test.go for setup)")
		}

		// Scale a hostgroup pod up and wait for a Host CR so workload placement
		// is independent of spec ordering (see messaging_test.go for the
		// rationale on why pod-Ready alone isn't enough).
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
		// Reaching Ready requires the host to pull the component, decode its
		// `(implements ..)` imports, and bind team-a/team-b — so the hostgroup
		// pod logs + pod state are the most direct evidence of where it broke
		// (image pull, decode, or plugin bind). The WorkloadDeployment/Workload
		// status conditions show whether placement vs sync vs health failed.
		dump("Pods", "get", "pods", "-n", namespace, "-o", "wide")
		dump("Events", "get", "events", "-n", namespace,
			"--sort-by=.lastTimestamp")
		dump("Hostgroup logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=600", "--prefix=true")
		dump("WorkloadDeployment", "get", "workloaddeployment", workloadName,
			"-n", namespace, "-o", "yaml")
		dump("WorkloadReplicaSets", "get", "workloadreplicasets.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
		dump("Workload CRs", "get", "workloads.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
		dump("Operator logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=runtime-operator", "--tail=200")
	})

	AfterAll(func() {
		if componentImage == "" {
			return
		}
		_ = exec.Command("kubectl", "delete", "workloaddeployment", workloadName,
			"-n", namespace, "--ignore-not-found=true").Run()
	})

	It("admits two named entries of the same package and keeps them distinct", func() {
		By("applying a WorkloadDeployment with team-a and team-b keyvalue imports")
		// Two entries of wasi:keyvalue under distinct names route to separate
		// backends; the unnamed http interface is the default route. Admission
		// must accept this (same package, distinct names) — the pre-feature
		// rules rejected duplicate namespace:package outright.
		manifest := fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: 1
  template:
    spec:
      hostInterfaces:
        - namespace: wasi
          package: http
          version: "0.2.2"
          interfaces:
            - incoming-handler
          config:
            host: keyvalue-implements
        - namespace: wasi
          package: keyvalue
          name: team-a
          version: "0.2.0-draft"
          interfaces:
            - store
          config:
            backend: in-memory
        - namespace: wasi
          package: keyvalue
          name: team-b
          version: "0.2.0-draft"
          interfaces:
            - store
          config:
            backend: in-memory
      components:
        - name: keyvalue-implements
          image: %s
`, workloadName, namespace, componentImage)

		cmd := exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(),
			"admission should accept two distinctly-named entries of wasi:keyvalue")

		By("waiting for the WorkloadDeployment to become Ready")
		Eventually(func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "workloaddeployment", workloadName,
				"-n", namespace,
				"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(output).To(Equal("True"))
		}).WithTimeout(3 * time.Minute).Should(Succeed())

		By("verifying both named interfaces survive onto the derived Workload CR")
		// The operator's EnsureHostInterface merges only entries sharing a
		// routing identity (namespace, package, name) and a compatible version.
		// team-a and team-b differ only by name, so they must NOT merge — both
		// names must be present on the materialized Workload.
		Eventually(func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
				"-n", namespace,
				"-o", "jsonpath={.items[*].spec.hostInterfaces[*].name}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			names := strings.Fields(output)
			g.Expect(names).To(ContainElement("team-a"))
			g.Expect(names).To(ContainElement("team-b"))
		}).WithTimeout(2 * time.Minute).Should(Succeed())
	})
})
