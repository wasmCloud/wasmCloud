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

// End-to-end coverage for `external-id` host interfaces: a WorkloadDeployment
// that binds wasi:keyvalue twice with **neither entry named**, telling them
// apart only by the resource each serves. Nothing in the manifest mentions
// `users` or `catalog` — the component's own labels for those imports.
//
// Reaching Ready exercises the whole stack: admission (an externalId-keyed
// entry is not the package's default route, so two are admissible),
// reconcile/dedup (EnsureHostInterface keeps them distinct), conversion (the
// controller copies externalId into the proto), and the host resolving each
// import by the external-id that stands in for its name.
//
// The byte-level isolation check lives in
// crates/wash-runtime/tests/integration_keyvalue_external_id.rs and the CEL
// rules in TestHostInterfacesAdmission (envtest). What only a cluster shows is
// the rest of the path: proto conversion, delivery to a real host, and that
// host binding the component.
//
// The component is the keyvalue-external-id fixture, built and served from the
// in-cluster registry (make e2e-images), so this spec runs only on the
// all-features leg — also the only host supporting the attribute — and
// self-skips otherwise. Runs in `make test-e2e`, not `make test`.
var _ = Describe("External-ID Host Interfaces", Ordered, func() {
	const (
		workloadName = "keyvalue-external-id"
		usersID      = "user-db-prod:region-a"
		catalogID    = "catalog-db-prod:region-a"
	)

	var componentImage string

	BeforeAll(func() {
		// Same gate as the implements spec: the fixture is served from the
		// in-cluster registry, and it needs a feature-enabled host to run at all
		// (the attribute rides the same `wasm_component_model_implements` gate).
		if !inClusterRegistry || !defaultHostAllFeatures {
			Skip("skipping external-id e2e (needs the in-cluster registry and an " +
				"all-features fixture host)")
		}
		componentImage = registryRef("keyvalue-external-id")

		ensureDefaultHostgroupReady()
	})

	AfterEach(func() {
		if !CurrentSpecReport().Failed() {
			return
		}
		dumpWorkloadDiagnostics(workloadName)
	})

	AfterAll(func() {
		if componentImage == "" {
			return
		}
		_ = exec.Command("kubectl", "delete", "workloaddeployment", workloadName,
			"-n", namespace, "--ignore-not-found=true").Run()
	})

	It("binds each import from its external-id alone", func() {
		By("applying a WorkloadDeployment whose keyvalue bindings carry only externalId")
		manifest := fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: 1
  template:
    spec:
      # Pin to the insecure default hostgroup; the registry hostgroup stays on
      # HTTPS and can't pull this fixture from the in-cluster (plain-HTTP) registry.
      hostSelector:
        hostgroup: default
      hostInterfaces:
        - namespace: wasi
          package: http
          version: "0.2.2"
          interfaces:
            - incoming-handler
          config:
            host: keyvalue-external-id
        - namespace: wasi
          package: keyvalue
          externalId: %q
          version: "0.2.0-draft"
          interfaces:
            - store
          config:
            backend: in-memory
        - namespace: wasi
          package: keyvalue
          externalId: %q
          version: "0.2.0-draft"
          interfaces:
            - store
          config:
            backend: in-memory
      components:
        - name: keyvalue-external-id
          image: %s
`, workloadName, namespace, usersID, catalogID, componentImage)

		cmd := exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(),
			"admission should accept two unnamed wasi:keyvalue entries with distinct externalIds")

		By("waiting for the WorkloadDeployment to become Ready")
		// Ready is the load-bearing assertion: the host can only instantiate the
		// component if it read both external-ids off the artifact and selected a
		// distinct binding for each. An unbound or ambiguous one fails the bind.
		Eventually(func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "workloaddeployment", workloadName,
				"-n", namespace,
				"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(output).To(Equal("True"))
		}).WithTimeout(3 * time.Minute).Should(Succeed())

		By("verifying both external-ids survive onto the derived Workload CR")
		// EnsureHostInterface merges only entries sharing a full routing identity.
		// These two differ only by externalId, so they must NOT collapse into one
		// — if they did, both imports would land on a single backend.
		Eventually(func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
				"-n", namespace,
				"-o", "jsonpath={.items[*].spec.hostInterfaces[*].externalId}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			ids := strings.Fields(output)
			g.Expect(ids).To(ContainElement(usersID))
			g.Expect(ids).To(ContainElement(catalogID))
		}).WithTimeout(2 * time.Minute).Should(Succeed())
	})
})
