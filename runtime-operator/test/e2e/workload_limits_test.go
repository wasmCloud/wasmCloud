package e2e

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os/exec"
	"strings"
	"sync"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

// End-to-end coverage for the instance limits a component declares —
// `poolSize`, `maxConcurrency`, `maxInvocations` — and the `replicas` above
// them. What the runtime does with each is covered in-process by
// crates/wash-runtime/tests/integration_instance_concurrency.rs, against
// `Component` structs built by hand; nothing there proves a limit survives the
// path a real deployment takes:
//
//	WorkloadDeployment CRD -> controller -> Workload CR -> proto -> host ->
//	InstancePolicy
//
// A limit that silently fails to arrive looks identical to one that arrived and
// was honoured, because the default for every one of them is the conservative
// value. `maxConcurrency` unset means one call at a time, which is exactly what
// a workload whose `maxConcurrency` never reached the host would do. This spec
// is what tells those two apart.
//
// The fixture (http-sleeper, P3) is the same one the in-process concurrency
// tests use, and it reports the two things that make the limits observable from
// outside the cluster, both timing-independent:
//
//   - `peak_in_flight`: the most calls this instance ever had running at once.
//     An instance serving one call at a time reports 1 however hard it is
//     driven, so a value above 1 can only come from `maxConcurrency` arriving.
//   - `served`: how many calls this instance has admitted. An instance retires
//     at `maxInvocations` and a fresh one starts over at 1, so a value above
//     the limit can only come from `maxInvocations` *not* arriving.
//
// Needs the in-cluster registry to pull the fixture from; self-skips otherwise.
// Excluded from `make test`; runs in the dedicated `make test-e2e` job.
var _ = Describe("Workload Limits", Ordered, func() {
	const (
		workloadName = "http-sleeper"
		// Gateway routes by Host header to the component's P3 handler export;
		// matches the `host` config on the wasi:http interface below.
		workloadHost = "http-sleeper.localhost.direct"

		replicas       = 2
		poolSize       = 2
		maxConcurrency = 4
		maxInvocations = 5

		// Enough concurrent requests to fill every warm instance a few times
		// over: replicas × poolSize × maxConcurrency is 16, and the fixture
		// parks on the clock rather than computing, so they genuinely overlap.
		burst = 24
	)

	var fixtureImage string

	BeforeAll(func() {
		if !inClusterRegistry {
			Skip("skipping workload limits e2e (needs the in-cluster registry)")
		}
		fixtureImage = registryRef(workloadName)
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
		// The Workload CRs carry the limits as the controller rendered them:
		// the first place to look when the host behaves like they are unset.
		dump("Workload CRs", "get", "workloads.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
		dump("Hostgroup logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=400", "--prefix=true")
	})

	AfterAll(func() {
		if fixtureImage == "" {
			return
		}
		_ = exec.Command("kubectl", "delete", "workloaddeployment", workloadName,
			"-n", namespace, "--ignore-not-found=true").Run()
	})

	It("carries a component's declared limits through to the runtime", func() {
		By("deploying a workload declaring every instance limit")
		manifest := fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: %d
  template:
    spec:
      # The default hostgroup is the insecure one, so it can pull the fixture
      # from the plain-HTTP in-cluster registry.
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
      components:
        - name: %s
          image: %s
          poolSize: %d
          maxConcurrency: %d
          maxInvocations: %d
`, workloadName, namespace, replicas, workloadHost, workloadName, fixtureImage,
			poolSize, maxConcurrency, maxInvocations)

		cmd := exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "failed to apply the WorkloadDeployment")

		By("waiting for the WorkloadDeployment to become Ready")
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("kubectl", "get", "workloaddeployment",
				workloadName, "-n", namespace,
				"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}"))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(out).To(Equal("True"))
		}).WithTimeout(3 * time.Minute).Should(Succeed())

		By("verifying replicas reached the controller")
		// `replicas` is the one limit observable without touching the guest.
		// `status.currentReplicas` is the controller's own count — the same
		// field the /scale subresource publishes for HPA.
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("kubectl", "get", "workloaddeployment",
				workloadName, "-n", namespace,
				"-o", "jsonpath={.status.currentReplicas}"))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(strings.TrimSpace(out)).To(Equal(fmt.Sprint(replicas)),
				"the deployment should report as many running instances as it asked for")
		}).WithTimeout(2 * time.Minute).Should(Succeed())

		By("waiting for the workload to serve")
		Eventually(func(g Gomega) {
			_, err := probeOnce(workloadHost)
			g.Expect(err).NotTo(HaveOccurred())
		}).WithTimeout(2 * time.Minute).Should(Succeed())

		By("driving a concurrent burst and reading what each instance reports")
		// Two rounds: the first fills the warm set (each instance pays the
		// fixture's one-off setup), the second is served by instances already
		// warm, which is where overlap shows up most clearly.
		var replies []sleeperReply
		for round := 0; round < 2; round++ {
			got, err := probeConcurrently(workloadHost, burst)
			Expect(err).NotTo(HaveOccurred(), "burst %d failed", round)
			replies = append(replies, got...)
		}
		Expect(replies).To(HaveLen(2*burst), "every request should have replied")

		peak, served := 0, 0
		for _, r := range replies {
			if r.PeakInFlight > peak {
				peak = r.PeakInFlight
			}
			if r.Served > served {
				served = r.Served
			}
		}
		_, _ = fmt.Fprintf(GinkgoWriter,
			"observed peak_in_flight=%d served=%d across %d replies\n",
			peak, served, len(replies))

		By("verifying maxConcurrency arrived")
		// Without it an instance takes one call at a time and reports 1 no
		// matter how hard it is driven, so this is what separates "the limit
		// reached the host" from "the limit was dropped somewhere".
		Expect(peak).To(BeNumerically(">", 1),
			"no instance ever overlapped calls — maxConcurrency did not reach the runtime")

		By("verifying maxConcurrency is also a bound")
		Expect(peak).To(BeNumerically("<=", maxConcurrency),
			"an instance exceeded maxConcurrency")

		By("verifying maxInvocations retires instances")
		// An instance stops admitting at the limit and a replacement starts
		// over at 1, so nothing may report more than the limit. The burst is
		// several times the limit, so an instance that never retired would
		// sail past it.
		Expect(served).To(BeNumerically("<=", maxInvocations),
			"an instance served more calls than maxInvocations allows")
		Expect(2*burst).To(BeNumerically(">", maxInvocations),
			"the burst must exceed maxInvocations for this to mean anything")
	})
})

// sleeperReply is what the http-sleeper fixture reports about the instance that
// served the request.
type sleeperReply struct {
	PeakInFlight int `json:"peak_in_flight"`
	Served       int `json:"served"`
}

// probeOnce sends one request through the gateway to the workload behind host.
func probeOnce(host string) (sleeperReply, error) {
	return probeWith(&http.Client{Timeout: 30 * time.Second}, host)
}

func probeWith(client *http.Client, host string) (sleeperReply, error) {
	var reply sleeperReply
	req, err := http.NewRequest(http.MethodGet, "http://localhost:80/", nil)
	if err != nil {
		return reply, err
	}
	// The gateway routes by Host header, not by URL.
	req.Host = host
	resp, err := client.Do(req)
	if err != nil {
		return reply, err
	}
	defer func() { _ = resp.Body.Close() }()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return reply, err
	}
	if resp.StatusCode != http.StatusOK {
		return reply, fmt.Errorf("status %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	if err := json.Unmarshal(body, &reply); err != nil {
		return reply, fmt.Errorf("decoding %q: %w", strings.TrimSpace(string(body)), err)
	}
	return reply, nil
}

// probeConcurrently sends n requests at once and returns every reply. The
// requests have to be genuinely in flight together — that is the whole point —
// so the client is allowed a connection per request rather than queueing them
// behind Go's default idle-connection cap.
func probeConcurrently(host string, n int) ([]sleeperReply, error) {
	client := &http.Client{
		Timeout: 60 * time.Second,
		Transport: &http.Transport{
			MaxIdleConns:        n,
			MaxIdleConnsPerHost: n,
			MaxConnsPerHost:     n,
		},
	}
	defer client.CloseIdleConnections()

	replies := make([]sleeperReply, n)
	errs := make([]error, n)
	var wg sync.WaitGroup
	wg.Add(n)
	for i := 0; i < n; i++ {
		go func(i int) {
			defer wg.Done()
			replies[i], errs[i] = probeWith(client, host)
		}(i)
	}
	wg.Wait()
	for i, err := range errs {
		if err != nil {
			return nil, fmt.Errorf("request %d: %w", i, err)
		}
	}
	return replies, nil
}
