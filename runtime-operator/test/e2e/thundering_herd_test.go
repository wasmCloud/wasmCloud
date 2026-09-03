package e2e

import (
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

// End-to-end coverage for a scheduling thundering herd: one WorkloadDeployment
// asking for a deployment's worth of replicas, all of which land on one host.
//
// The herd is real rather than arranged. `reconcileScaleUp` creates every
// missing Workload in one pass, so all `replicas` CRs appear together; and
// `findFreeHost` shuffles the schedulable hosts and takes the first *available*
// one, with no capacity accounting of any kind — so with a single-replica
// hostgroup every one of them is placed on the same host, whatever the herd's
// size.
//
// What the layers below already cover, and why neither substitutes for this:
//
//   - crates/wash-runtime/tests/integration_thundering_herd.rs drives `HostApi`
//     directly, so the whole herd starts at once. That is the host's own
//     concurrency, with no admission control in front of it.
//   - integration_washlet_api.rs covers the NATS command path — the
//     `max_concurrent_starts` permit, and that heartbeats survive a burst — but
//     every start there hangs on a stalling registry, so nothing in it reaches
//     Running.
//
// Neither runs in a container. This is the only place the herd meets the
// resource limits a real host pod is given, and those limits are what decide
// how much of it runs at once: `max_concurrent_starts` is
// `available_parallelism() - 1` clamped to 4, and `available_parallelism()`
// reads the pod's CPU quota — 500m here, so the host works through the herd
// one start at a time. The question this spec answers is whether the deployment
// still converges, and whether the host stays heard from while it does. A host
// that stops heartbeating is deleted by the operator, and every workload on it
// goes too — so a herd that silences its own host takes out the very workloads
// it was placing.
// herdWorkload is one Workload CR of a herd, reduced to what the spec asserts on.
type herdWorkload struct {
	name   string
	ready  string
	hostID string
}

// herdHostGroup is the host group the herd is placed on. Every reading below
// is scoped to it: the `registry` group beside it runs a different workload on
// a different image and its health is not this spec's subject.
const herdHostGroup = "default"

// herdHostPods selects the pods of the host group the herd lands on.
var herdHostPods = "wasmcloud.com/name=hostgroup,wasmcloud.com/hostgroup=" + herdHostGroup

// hostPodRestarts maps each of the herd host group's pods to how many times its
// containers have restarted.
//
// Read as a delta rather than an absolute: a host pod exits when NATS is not up
// yet ("failed to connect to NATS Scheduler URL"), so a freshly installed
// release routinely carries a restart or two that has nothing to do with any
// workload.
func hostPodRestarts() map[string]string {
	out, err := utils.Run(exec.Command("kubectl", "get", "pods", "-n", namespace,
		"-l", herdHostPods,
		"-o", `jsonpath={range .items[*]}{.metadata.name}{"\t"}{.status.containerStatuses[*].restartCount}{"\t"}{.status.containerStatuses[*].lastState.terminated.reason}{"\n"}{end}`))
	if err != nil {
		return nil
	}
	restarts := map[string]string{}
	for _, line := range strings.Split(out, "\n") {
		fields := strings.SplitN(line, "\t", 3)
		if len(fields) < 3 {
			continue
		}
		restarts[fields[0]] = fields[1] + " (last termination: " + fields[2] + ")"
	}
	return restarts
}

// herdWorkloads returns the Workload CRs belonging to `deployment`, matched on
// the `<deployment>-<replicaset hash>-<workload hash>` name the controllers
// generate. Nothing labels a Workload with the deployment it came from, and
// listing the namespace unfiltered would pick up whatever another spec left.
func herdWorkloads(deployment string) []herdWorkload {
	out, err := utils.Run(exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
		"-n", namespace,
		"-o", `jsonpath={range .items[*]}{.metadata.name}{"\t"}{.status.conditions[?(@.type=="Ready")].status}{"\t"}{.status.hostID}{"\n"}{end}`))
	if err != nil {
		return nil
	}
	var rows []herdWorkload
	for _, line := range strings.Split(out, "\n") {
		// Split to a fixed width rather than trimming: a row whose workload is
		// placed but not yet Ready ends in empty fields, and trimming the line
		// would drop them and take the row with them.
		fields := strings.SplitN(line, "\t", 3)
		if len(fields) < 3 || !strings.HasPrefix(fields[0], deployment+"-") {
			continue
		}
		rows = append(rows, herdWorkload{name: fields[0], ready: fields[1], hostID: fields[2]})
	}
	return rows
}

var _ = Describe("Thundering Herd", Ordered, func() {
	const (
		deploymentName = "herd"
		workloadHost   = "herd.localhost.direct"
		// The published hello-world component, so this spec does not need the
		// in-cluster registry and runs on every leg.
		image = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0"

		// Generous: the host serializes the herd's compiles on half a core.
		// This is a convergence bound, not a performance one — what it rules
		// out is a herd that never finishes.
		converge = 5 * time.Minute
	)

	// A deployment's worth, several times the host's start permit.
	// E2E_HERD_REPLICAS raises it to probe where convergence starts to hurt,
	// without committing CI to the cost of finding out every run.
	//
	// Parsed here rather than with a Gomega assertion: this runs while Ginkgo
	// builds the spec tree, where `Fail` is not valid and aborts the whole
	// suite with a message about the wrong thing.
	replicas := 15
	if v := os.Getenv("E2E_HERD_REPLICAS"); v != "" {
		parsed, err := strconv.Atoi(v)
		if err != nil {
			panic(fmt.Sprintf("E2E_HERD_REPLICAS must be a number, got %q: %v", v, err))
		}
		replicas = parsed
	}

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
		dump("Hosts", "get", "hosts.runtime.wasmcloud.dev", "-n", namespace, "-o", "wide")
		dump("Workloads", "get", "workloads.runtime.wasmcloud.dev", "-n", namespace, "-o", "wide")
		dump("Events", "get", "events", "-n", namespace, "--sort-by=.lastTimestamp")
		dump("Hostgroup logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=400", "--prefix=true")
	})

	AfterAll(func() {
		_ = exec.Command("kubectl", "delete", "workloaddeployment", deploymentName,
			"-n", namespace, "--ignore-not-found=true").Run()
	})

	It("places every replica of a herd on one host and converges", func() {
		// Earlier specs upgrade the release, which rolls this Deployment. Until
		// that settles there are two generations of host pod about — one
		// draining, one starting — and neither is Ready, which is a rolling
		// update rather than anything the herd did.
		By("waiting for the host group to finish rolling out")
		_, err := utils.Run(exec.Command("kubectl", "rollout", "status",
			fmt.Sprintf("deployment/hostgroup-%s", herdHostGroup),
			"-n", namespace, "--timeout=3m"))
		Expect(err).NotTo(HaveOccurred(), "the host group never finished rolling out")

		// The baseline the watch below is against. Without it a host that has
		// not finished registering reads the same as one that has gone quiet:
		// both have no Ready condition to report.
		By("waiting for the host to be Ready before the herd arrives")
		var hostName string
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("kubectl", "get",
				"hosts.runtime.wasmcloud.dev", "-n", namespace,
				"-l", "hostgroup=default",
				"-o", `jsonpath={range .items[*]}{.metadata.name}={.status.conditions[?(@.type=="Ready")].status} {end}`))
			g.Expect(err).NotTo(HaveOccurred())
			for _, entry := range strings.Fields(out) {
				if name, ok := strings.CutSuffix(entry, "=True"); ok {
					hostName = name
					return
				}
			}
			g.Expect(out).To(BeEmpty(), "no host in the default hostgroup is Ready yet")
		}).WithTimeout(3 * time.Minute).WithPolling(2 * time.Second).Should(Succeed())

		// The baseline for the restart check after convergence.
		restartsBefore := hostPodRestarts()
		Expect(restartsBefore).NotTo(BeEmpty(), "no hostgroup pod to watch")

		By(fmt.Sprintf("applying a %d-replica WorkloadDeployment", replicas))
		manifest := fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: %d
  template:
    spec:
      hostSelector:
        hostgroup: default
      hostInterfaces:
        - namespace: wasi
          package: http
          interfaces:
            - incoming-handler
          config:
            host: %s
      components:
        - name: hello-world
          image: %s
`, deploymentName, namespace, replicas, workloadHost, image)

		cmd := exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err = utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "failed to apply the herd WorkloadDeployment")

		// Watched for the whole convergence rather than sampled after it: both
		// failures this guards against are transient by nature. A host that
		// goes quiet long enough to be reaped and then comes back looks healthy
		// afterwards, and every workload it was carrying is gone; a pod whose
		// `/readyz` refused during the herd is back in the Service by the time
		// anything reads it, having dropped whatever arrived meanwhile.
		var (
			mu       sync.Mutex
			notReady []string
			done     = make(chan struct{})
			watching sync.WaitGroup
		)
		watching.Add(1)
		// Deferred rather than closed after the wait below: a convergence that
		// times out unwinds the spec through a panic, and a watcher whose only
		// exit is this channel would then keep shelling out to kubectl for the
		// rest of the suite — on the failure path, which is the one that matters.
		defer func() {
			select {
			case <-done:
			default:
				close(done)
			}
			watching.Wait()
		}()
		go func() {
			defer watching.Done()
			defer GinkgoRecover()
			for {
				select {
				case <-done:
					return
				case <-time.After(2 * time.Second):
				}
				// The host the herd is landing on, by name. `--ignore-not-found`
				// so a reaped Host CR comes back as empty output rather than a
				// non-zero exit: being gone is the failure this watches for, and
				// skipping errors would skip exactly that.
				out, err := utils.Run(exec.Command("kubectl", "get",
					"hosts.runtime.wasmcloud.dev", hostName, "-n", namespace,
					"--ignore-not-found",
					"-o", `jsonpath={.status.conditions[?(@.type=="Ready")].status}`))
				if err != nil {
					continue
				}
				if out != "True" {
					mu.Lock()
					notReady = append(notReady, fmt.Sprintf("host CR Ready=%q at %s", out, time.Now().Format(time.RFC3339)))
					mu.Unlock()
				}

				// The pod's own readiness, which is what Kubernetes acts on:
				// `/readyz` drives it, and it goes red when the host is draining
				// or its ingress is at its ceiling. A herd that pushes the host
				// past that ceiling takes its endpoint out of the Service, so
				// the workloads it just started are unreachable through the
				// gateway — while every workload still reports Ready.
				pods, err := utils.Run(exec.Command("kubectl", "get", "pods", "-n", namespace,
					"-l", herdHostPods,
					"--field-selector=status.phase=Running",
					"-o", `jsonpath={range .items[*]}{.metadata.name}={.status.conditions[?(@.type=="Ready")].status} {end}`))
				if err != nil {
					continue
				}
				for _, entry := range strings.Fields(pods) {
					name, ready, _ := strings.Cut(entry, "=")
					// Only the pods the herd is actually running on. A pod that
					// appears mid-herd belongs to a rollout somebody else
					// started, and its two generations are both legitimately
					// unready; the restart check is what catches a replacement
					// this herd caused.
					if _, ours := restartsBefore[name]; !ours || ready == "True" {
						continue
					}
					mu.Lock()
					notReady = append(notReady, fmt.Sprintf("pod %s=%s at %s", name, ready, time.Now().Format(time.RFC3339)))
					mu.Unlock()
				}
			}
		}()

		By("waiting for the WorkloadDeployment to become Ready")
		started := time.Now()
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("kubectl", "get", "workloaddeployment",
				deploymentName, "-n", namespace,
				"-o", `jsonpath={.status.conditions[?(@.type=="Ready")].status}`))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(out).To(Equal("True"))
		}).WithTimeout(converge).WithPolling(2 * time.Second).Should(Succeed())
		converged := time.Since(started)
		close(done)
		watching.Wait()

		// How wide the herd actually ran, read off the host rather than
		// inferred: the washlet logs the permit it resolved from the pod's CPU
		// quota in its "Host started" line.
		width := "unknown"
		if out, err := utils.Run(exec.Command("kubectl", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=-1")); err == nil {
			for _, line := range strings.Split(out, "\n") {
				if !strings.Contains(line, "Host started") {
					continue
				}
				if _, rest, ok := strings.Cut(line, "max_concurrent_starts="); ok {
					width, _, _ = strings.Cut(rest, " ")
				}
			}
		}
		_, _ = fmt.Fprintf(GinkgoWriter,
			"herd of %d converged in %s (host admitted %s start(s) at a time)\n",
			replicas, converged.Round(time.Second), width)

		mu.Lock()
		observed := append([]string(nil), notReady...)
		mu.Unlock()
		Expect(observed).To(BeEmpty(),
			"host %s stopped reporting Ready while the herd was starting. The Host CR going quiet means the operator deletes it and every workload on it; the pod going unready means `/readyz` refused and the endpoint left the Service, so the workloads it just started are unreachable",
			hostName)

		// Compilation is the memory peak of a start, and the pod is capped at
		// 512Mi. A herd that walks the pod into an OOMKill restarts the host,
		// which takes every workload on it — and the deployment then converges
		// anyway on the way back up, so readiness alone would not notice.
		By("verifying the herd did not restart the host pod")
		restartsAfter := hostPodRestarts()
		// Every pod that was there before must still be there. Ranging over the
		// second reading alone would report success from an empty map — a
		// kubectl blip, or a herd that took the whole host group down, both
		// read as "nothing restarted".
		for pod, before := range restartsBefore {
			after, still := restartsAfter[pod]
			Expect(still).To(BeTrue(), "host pod %s is gone after the herd; it was at %s before", pod, before)
			Expect(after).To(Equal(before), "host pod %s restarted during the herd: %s, was %s", pod, after, before)
		}
		for pod := range restartsAfter {
			Expect(restartsBefore).To(HaveKey(pod),
				"host pod %s appeared during the herd, replacing the one it started on", pod)
		}

		By("verifying every replica reached Ready, on one host")
		// The Workload CRs carry no label naming their deployment, so they are
		// matched on the generated `<replicaset>-<hash>` name, which is
		// prefixed with it.
		rows := herdWorkloads(deploymentName)
		Expect(rows).To(HaveLen(replicas), "expected %d Workload CRs, got %d", replicas, len(rows))

		hosts := map[string]int{}
		for _, row := range rows {
			Expect(row.ready).To(Equal("True"), "%s is not Ready", row.name)
			hosts[row.hostID]++
		}
		Expect(hosts).To(HaveLen(1),
			"the herd was spread over %d hosts, so this run did not test a herd on one host: %v", len(hosts), hosts)
		for id, count := range hosts {
			Expect(count).To(Equal(replicas), "host %s carries %d of %d replicas", id, count, replicas)
		}
	})

	It("still serves HTTP through the gateway after the herd", func() {
		Eventually(func(g Gomega) {
			out, err := utils.Run(exec.Command("curl", "-s", "-o", "/dev/null",
				"-w", "%{http_code}",
				"-H", fmt.Sprintf("Host: %s", workloadHost),
				gatewayURL("")))
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(out).To(Equal("200"))
		}).WithTimeout(2 * time.Minute).WithPolling(2 * time.Second).Should(Succeed())
	})

	It("tears the whole herd down on delete", func() {
		By("deleting the WorkloadDeployment")
		_, err := utils.Run(exec.Command("kubectl", "delete", "workloaddeployment",
			deploymentName, "-n", namespace))
		Expect(err).NotTo(HaveOccurred())

		By("waiting for every Workload CR to go")
		Eventually(func(g Gomega) {
			g.Expect(herdWorkloads(deploymentName)).To(BeEmpty())
		}).WithTimeout(3 * time.Minute).WithPolling(2 * time.Second).Should(Succeed())

		By("verifying the host survived the herd")
		out, err := utils.Run(exec.Command("kubectl", "get", "hosts.runtime.wasmcloud.dev",
			"-n", namespace, "-l", "hostgroup=default",
			"-o", `jsonpath={range .items[*]}{.status.conditions[?(@.type=="Ready")].status} {end}`))
		Expect(err).NotTo(HaveOccurred())
		Expect(strings.Fields(out)).To(ContainElement("True"),
			"no host in the default hostgroup is Ready after the herd")
	})
})
