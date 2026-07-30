/*
Copyright 2024.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

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

// Regression coverage for https://github.com/wasmCloud/wasmCloud/issues/5074:
// a WorkloadDeployment that exports wasmcloud:messaging/handler with a
// `subscriptions` config must register a NATS subscription on the data bus.
// Before the fix, the WorkloadDeployment reached Ready=True but no SUB ever
// landed on NATS, so requests on the configured subject silently timed out.
//
// The round trip runs once per messaging revision (see messagingRevisions), so
// the same assertions cover the sync `@0.2.0` surface and the async `@0.3.0`
// one. Both fixtures echo the request body back on msg.reply_to; they differ in
// which WIT revision they were built against, and — because `@0.3.0`'s
// handle-message is an `async func` — in which host delivery path serves them.
// Like every e2e fixture they are built and served from the in-cluster registry
// (make e2e-images) rather than published images; they run on any host, so this
// runs on both wash.yml legs (the release and all-features fixture hosts) and
// self-skips only where the registry flow is off.
//
// On failure, the spec dumps hostgroup pod logs (with RUST_LOG bumped to
// debug for `wash_runtime`) so the NatsMessaging plugin's instrumentation
// makes it possible to localize where the resolve path broke.

// messagingRevision is one (fixture, WIT revision) pair to run the round trip
// against. Each gets its own workload, subject, and client pod so the cases stay
// independent regardless of Ginkgo's spec ordering.
type messagingRevision struct {
	// label distinguishes the specs in Ginkgo output.
	label string
	// fixture is the registry image name (see the FIXTURES list in
	// xtask/src/e2e_images.rs).
	fixture string
	// witVersion goes on the WorkloadDeployment's hostInterfaces entry. It is
	// what selects the host's sync or async messaging surface, so it is the
	// single meaningful difference between these cases.
	witVersion string
	subject    string
	payload    string
}

var messagingRevisions = []messagingRevision{
	{
		label:      "sync wasmcloud:messaging@0.2.0",
		fixture:    "messaging-echo",
		witVersion: "0.2.0",
		subject:    "test.echo",
		payload:    "ping-5074",
	},
	{
		// The async surface: handle-message and publish are `async func`s, so
		// the guest awaits its reply from inside the handler and the host drives
		// the export through the concurrent ABI.
		label:      "async wasmcloud:messaging@0.3.0",
		fixture:    "messaging-echo-p3",
		witVersion: "0.3.0",
		subject:    "test.echo.p3",
		payload:    "ping-async-030",
	},
}

// workloadName / podName derive per-revision names from the fixture so the two
// cases never share a WorkloadDeployment or client pod.
func (r messagingRevision) workloadName() string { return r.fixture }
func (r messagingRevision) podName() string      { return "nats-echo-client-" + r.fixture }

var _ = Describe("Messaging Subscription", Ordered, func() {
	BeforeAll(func() {
		// The messaging fixtures are built and served from the in-cluster
		// registry (make e2e-images), like every other e2e fixture, and they run on
		// any host — so this spec runs on both wash.yml legs (the release and
		// all-features fixture hosts both pull them from the registry). It self-skips
		// only where the registry flow is off (the canary job, plain local runs).
		if !inClusterRegistry {
			Skip("in-cluster registry disabled; skipping messaging e2e")
		}

		// Earlier specs (Finalizer) may have scaled the hostgroup to zero;
		// scale back up and wait for a host to be Ready so this spec is
		// independent of test ordering.
		//
		// NOTE: deliberately *not* bumping RUST_LOG via `kubectl set env`
		// here — that triggers a rolling update, and during the brief window
		// where both old and new pods exist, workload assignment races and
		// can produce flaky "no responders" failures unrelated to the bug
		// under test. If you need plugin debug logs, either (a) re-run
		// after the test fails to inspect the still-running pod, or (b)
		// rebuild the chart with a runtime.podSpec.containers[].env override.
		By("ensuring at least one hostgroup pod is running")
		cmd := exec.Command("kubectl", "scale", "deployment/hostgroup-default",
			"--replicas=1", "-n", namespace)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "Failed to scale hostgroup")

		cmd = exec.Command("kubectl", "rollout", "status",
			"-n", namespace,
			"deployment/hostgroup-default",
			"--timeout=2m")
		_, err = utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "hostgroup rollout did not complete")

		// `kubectl rollout status` only waits for the Kubernetes Deployment
		// to come up, pod Ready means the wash binary is running, not that
		// it has connected to NATS, sent a heartbeat, and had a Host CR
		// registered for it. Under Ginkgo's default randomized spec order
		// this matters: when the messaging spec runs first (immediately
		// after Helm install), the Host CR may not exist yet, so the
		// Workload reconciler reports "no suitable host found" and the
		// workload silently never lands on a host.Wait for an actual Host
		// so the rest of this spec can trust that workload placement will succeed.
		By("waiting for a Host CR to be registered")
		verifyHostRegistered := func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "hosts.runtime.wasmcloud.dev",
				"-n", namespace, "-o", "jsonpath={.items}")
			output, err := utils.Run(cmd)
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(output).NotTo(Equal("[]"), "no Host CR registered yet")
		}
		Eventually(verifyHostRegistered).WithTimeout(2 * time.Minute).Should(Succeed())
	})

	AfterEach(func() {
		// On failure, dump everything that might explain why the messaging
		// round-trip didn't complete: host logs (with debug instrumentation),
		// operator logs, pod state, and the relevant CRs. The bug in #5074
		// surfaces in any of: host failed to bind, host bound but didn't
		// subscribe, or the WD never landed on a host at all.
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
		// The client pod logs are the most direct evidence of what the
		// CLI saw — empty body, "no responders", connection error, etc.
		// The Gomega failure message also embeds them, but pasting only
		// part of the output is common, so dump them under their own
		// header to make sure they always appear in the diagnostic block.
		// Dumped for every revision: which spec failed is not known here, and
		// a pod that was never created just reports as missing.
		for _, rev := range messagingRevisions {
			dump(rev.podName()+" logs", "logs", rev.podName(), "-n", namespace)
			dump(rev.podName()+" describe", "describe", "pod", rev.podName(), "-n", namespace)
			dump("WorkloadDeployment "+rev.workloadName(), "get", "workloaddeployment",
				rev.workloadName(), "-n", namespace, "-o", "yaml")
		}
		dump("Hostgroup logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=600", "--prefix=true")
		dump("Operator logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=runtime-operator", "--tail=200")
		dump("Workload CRs", "get", "workloads.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
		dump("Host CRs", "get", "hosts.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
	})

	AfterAll(func() {
		if !inClusterRegistry {
			return
		}
		// Best-effort cleanup; ignore errors so the suite teardown isn't
		// derailed by an already-deleted resource.
		for _, rev := range messagingRevisions {
			_ = exec.Command("kubectl", "delete", "workloaddeployment", rev.workloadName(),
				"-n", namespace, "--ignore-not-found=true").Run()
			_ = exec.Command("kubectl", "delete", "pod", rev.podName(),
				"-n", namespace, "--ignore-not-found=true").Run()
		}
	})

	for _, rev := range messagingRevisions {
		It("should register the NATS subscription and round-trip a request ("+rev.label+")", func() {
			By("applying the messaging WorkloadDeployment")
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
        - namespace: wasmcloud
          package: messaging
          version: "%s"
          interfaces:
            - handler
          config:
            subscriptions: "%s"
      components:
        - name: %s
          image: %s
`, rev.workloadName(), namespace, rev.witVersion, rev.subject, rev.fixture, registryRef(rev.fixture))

			cmd := exec.Command("kubectl", "apply", "-f", "-")
			cmd.Stdin = strings.NewReader(manifest)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to apply messaging WorkloadDeployment")

			By("waiting for WorkloadDeployment to become Ready")
			verifyWorkloadReady := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "workloaddeployment", rev.workloadName(),
					"-n", namespace,
					"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("True"))
			}
			Eventually(verifyWorkloadReady).WithTimeout(3 * time.Minute).Should(Succeed())

			// Note: reaching Ready=True is necessary but not sufficient — the bug
			// exhibits exactly that state. The next probe is the real assertion.

			By("issuing a NATS request to the subscribed subject from inside the cluster")
			// Run a one-shot nats-box pod against the in-cluster NATS service. If
			// the handler subscribed successfully, the component echoes the body
			// on reply_to and `nats request` prints it. If the bug is present, no
			// responder exists and nats fails with "no responders".
			//
			// The chart enables TLS + mTLS by default (global.tls.enabled=true),
			// so the pod mounts the cluster-generated data-plane cert secret and
			// passes the cert / key / CA to the nats CLI. The volume is marked
			// optional so the spec still runs if someone disables TLS via helm
			// override; the empty mount makes nats CLI fail with a clear error
			// rather than a silent verify-skip.
			echoPayload := rev.payload
			podName := rev.podName()
			podManifest := fmt.Sprintf(`apiVersion: v1
kind: Pod
metadata:
  name: %s
  namespace: %s
spec:
  restartPolicy: Never
  containers:
    - name: nats
      image: natsio/nats-box:latest
      command:
        - nats
        - --server=nats://nats:4222
        - --tlsca=/data-cert/ca.crt
        - --tlscert=/data-cert/tls.crt
        - --tlskey=/data-cert/tls.key
        - request
        - --timeout=10s
        - %s
        - %s
      volumeMounts:
        - name: data-cert
          mountPath: /data-cert
          readOnly: true
  volumes:
    - name: data-cert
      secret:
        secretName: wasmcloud-data-tls
        optional: true
`, podName, namespace, rev.subject, echoPayload)

			cmd = exec.Command("kubectl", "apply", "-f", "-")
			cmd.Stdin = strings.NewReader(podManifest)
			_, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to create nats-echo-client pod")

			By("waiting for nats-echo-client pod to terminate")
			verifyTerminated := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "pod", podName,
					"-n", namespace,
					"-o", "jsonpath={.status.phase}")
				phase, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(phase).To(Or(Equal("Succeeded"), Equal("Failed")),
					"pod still %s", phase)
			}
			Eventually(verifyTerminated).WithTimeout(30 * time.Second).Should(Succeed())

			By("collecting the NATS client pod logs")
			cmd = exec.Command("kubectl", "logs", podName, "-n", namespace)
			output, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to fetch NATS client pod logs")

			// Phase=Succeeded is the strongest signal the round trip worked, since
			// the nats CLI exits non-zero on "no responders" or timeout. We still
			// assert the payload appears in the reply for an extra sanity check.
			cmd = exec.Command("kubectl", "get", "pod", podName,
				"-n", namespace, "-o", "jsonpath={.status.phase}")
			phase, _ := utils.Run(cmd)
			Expect(phase).To(Equal("Succeeded"),
				"nats request did not succeed — handler subscription likely never "+
					"registered (regression of #5074). pod logs:\n%s", output)
			Expect(output).To(ContainSubstring(echoPayload),
				"handler did not echo the request body back; actual reply:\n%s", output)
		})
	}
})
