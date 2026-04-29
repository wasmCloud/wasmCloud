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
	"os"
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
// To run this test fully, two pieces of infrastructure are required:
//
//  1. MESSAGING_E2E_IMAGE — an OCI ref the cluster can pull, pointing at a
//     wasm component that exports wasmcloud:messaging/handler@0.2.0 and
//     replies to incoming messages by publishing the body back on
//     msg.reply_to. The fixture under
//     crates/wash-runtime/tests/fixtures/messaging-handler does exactly that;
//     publish it (e.g. `ghcr.io/wasmcloud/components/messaging-echo-rust:0.1.0`).
//  2. BUILD_RUNTIME_IMAGE=true — builds the wash-runtime (host) image from
//     the local tree so the host pod actually exercises the code under
//     test. Without this the e2e runs against the published canary image.
//
// On failure, the spec dumps hostgroup pod logs (with RUST_LOG bumped to
// debug for `wash_runtime`) so the NatsMessaging plugin's instrumentation
// makes it possible to localize where the resolve path broke.
var _ = Describe("Messaging Subscription", Ordered, func() {
	const subscriptionSubject = "test.echo"
	const workloadName = "messaging-echo"

	var componentImage string

	BeforeAll(func() {
		componentImage = os.Getenv("MESSAGING_E2E_IMAGE")
		if componentImage == "" {
			Skip("MESSAGING_E2E_IMAGE not set; skipping messaging e2e " +
				"(see runtime-operator/test/e2e/messaging_test.go for setup)")
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
		dump("Hostgroup logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=hostgroup", "--tail=600", "--prefix=true")
		dump("Operator logs", "logs", "-n", namespace,
			"-l", "wasmcloud.com/name=runtime-operator", "--tail=200")
		dump("WorkloadDeployment", "get", "workloaddeployment", workloadName,
			"-n", namespace, "-o", "yaml")
		dump("Workload CRs", "get", "workloads.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
		dump("Host CRs", "get", "hosts.runtime.wasmcloud.dev",
			"-n", namespace, "-o", "yaml")
	})

	AfterAll(func() {
		if componentImage == "" {
			return
		}
		// Best-effort cleanup; ignore errors so the suite teardown isn't
		// derailed by an already-deleted resource.
		_ = exec.Command("kubectl", "delete", "workloaddeployment", workloadName,
			"-n", namespace, "--ignore-not-found=true").Run()
		_ = exec.Command("kubectl", "delete", "pod", "nats-echo-client",
			"-n", namespace, "--ignore-not-found=true").Run()
	})

	It("should register the NATS subscription and round-trip a request", func() {
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
      hostInterfaces:
        - namespace: wasmcloud
          package: messaging
          version: "0.2.0"
          interfaces:
            - handler
          config:
            subscriptions: "%s"
      components:
        - name: messaging-echo
          image: %s
`, workloadName, namespace, subscriptionSubject, componentImage)

		cmd := exec.Command("kubectl", "apply", "-f", "-")
		cmd.Stdin = strings.NewReader(manifest)
		_, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "Failed to apply messaging WorkloadDeployment")

		By("waiting for WorkloadDeployment to become Ready")
		verifyWorkloadReady := func(g Gomega) {
			cmd := exec.Command("kubectl", "get", "workloaddeployment", workloadName,
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
		const echoPayload = "ping-5074"
		const podName = "nats-echo-client"
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
`, podName, namespace, subscriptionSubject, echoPayload)

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

		By("collecting nats-echo-client logs")
		cmd = exec.Command("kubectl", "logs", podName, "-n", namespace)
		output, err := utils.Run(cmd)
		Expect(err).NotTo(HaveOccurred(), "Failed to fetch nats-echo-client logs")

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
})
