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
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

// namespace where the project is deployed in
const namespace = "wasmcloud-system"

// serviceAccountName created for the project
const serviceAccountName = "operator-controller-manager"

// metricsServiceName is the name of the metrics service of the project
const metricsServiceName = "operator-controller-manager-metrics-service"

// metricsRoleBindingName is the name of the RBAC that will be created to allow get the metrics data
const metricsRoleBindingName = "operator-metrics-binding"

var _ = Describe("Manager", Ordered, func() {
	var controllerPodName string

	// After each test, check for failures and collect logs, events,
	// and pod descriptions for debugging.
	AfterEach(func() {
		specReport := CurrentSpecReport()
		if specReport.Failed() {
			By("Fetching controller manager pod logs")
			cmd := exec.Command("kubectl", "logs", "-n", namespace,
				"-l", "wasmcloud.com/name=runtime-operator", "--tail=200")
			controllerLogs, err := utils.Run(cmd)
			if err == nil {
				_, _ = fmt.Fprintf(GinkgoWriter, "Controller logs:\n %s", controllerLogs)
			} else {
				_, _ = fmt.Fprintf(GinkgoWriter, "Failed to get Controller logs: %s", err)
			}

			By("Fetching Kubernetes events")
			cmd = exec.Command("kubectl", "get", "events", "-n", namespace, "--sort-by=.lastTimestamp")
			eventsOutput, err := utils.Run(cmd)
			if err == nil {
				_, _ = fmt.Fprintf(GinkgoWriter, "Kubernetes events:\n%s", eventsOutput)
			} else {
				_, _ = fmt.Fprintf(GinkgoWriter, "Failed to get Kubernetes events: %s", err)
			}

			By("Fetching all pod status")
			cmd = exec.Command("kubectl", "get", "pods", "-n", namespace, "-o", "wide")
			podOutput, err := utils.Run(cmd)
			if err == nil {
				_, _ = fmt.Fprintf(GinkgoWriter, "Pod status:\n%s", podOutput)
			} else {
				_, _ = fmt.Fprintf(GinkgoWriter, "Failed to get pod status: %s", err)
			}
		}
	})

	SetDefaultEventuallyTimeout(2 * time.Minute)
	SetDefaultEventuallyPollingInterval(time.Second)

	Context("Manager", func() {
		It("should run successfully", func() {
			By("validating that the controller-manager pod is running as expected")
			verifyControllerUp := func(g Gomega) {
				cmd := exec.Command("kubectl", "get",
					"pods", "-l", "wasmcloud.com/name=runtime-operator",
					"-o", "go-template={{ range .items }}"+
						"{{ if not .metadata.deletionTimestamp }}"+
						"{{ .metadata.name }}"+
						"{{ \"\\n\" }}{{ end }}{{ end }}",
					"-n", namespace,
				)

				podOutput, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred(), "Failed to retrieve controller-manager pod information")
				podNames := utils.GetNonEmptyLines(podOutput)
				g.Expect(podNames).To(HaveLen(1), "expected 1 controller pod running")
				controllerPodName = podNames[0]

				// Validate the pod's status
				cmd = exec.Command("kubectl", "get",
					"pods", controllerPodName, "-o", "jsonpath={.status.phase}",
					"-n", namespace,
				)
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("Running"), "Incorrect controller-manager pod status")
			}
			Eventually(verifyControllerUp).Should(Succeed())
		})

		It("should ensure the metrics endpoint is serving metrics", func() {
			if skipPrometheusInstall {
				Skip("Prometheus not installed, skipping metrics test")
			}

			By("creating a ClusterRoleBinding for the service account to allow access to metrics")
			cmd := exec.Command("kubectl", "create", "clusterrolebinding", metricsRoleBindingName,
				"--clusterrole=operator-metrics-reader",
				fmt.Sprintf("--serviceaccount=%s:%s", namespace, serviceAccountName),
			)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to create ClusterRoleBinding")

			By("validating that the metrics service is available")
			cmd = exec.Command("kubectl", "get", "service", metricsServiceName, "-n", namespace)
			_, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Metrics service should exist")

			By("validating that the ServiceMonitor for Prometheus is applied in the namespace")
			cmd = exec.Command("kubectl", "get", "ServiceMonitor", "-n", namespace)
			_, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "ServiceMonitor should exist")

			By("getting the service account token")
			token, err := serviceAccountToken()
			Expect(err).NotTo(HaveOccurred())
			Expect(token).NotTo(BeEmpty())

			By("waiting for the metrics endpoint to be ready")
			verifyMetricsEndpointReady := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "endpoints", metricsServiceName, "-n", namespace)
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(ContainSubstring("8443"), "Metrics endpoint is not ready")
			}
			Eventually(verifyMetricsEndpointReady).Should(Succeed())

			By("verifying that the controller manager is serving the metrics server")
			verifyMetricsServerStarted := func(g Gomega) {
				cmd := exec.Command("kubectl", "logs", controllerPodName, "-n", namespace)
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(ContainSubstring("controller-runtime.metrics\tServing metrics server"),
					"Metrics server not yet started")
			}
			Eventually(verifyMetricsServerStarted).Should(Succeed())

			By("creating the curl-metrics pod to access the metrics endpoint")
			cmd = exec.Command("kubectl", "run", "curl-metrics", "--restart=Never",
				"--namespace", namespace,
				"--image=curlimages/curl:7.78.0",
				"--", "/bin/sh", "-c", fmt.Sprintf(
					"curl -v -k -H 'Authorization: Bearer %s' https://%s.%s.svc.cluster.local:8443/metrics",
					token, metricsServiceName, namespace))
			_, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to create curl-metrics pod")

			By("waiting for the curl-metrics pod to complete.")
			verifyCurlUp := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "pods", "curl-metrics",
					"-o", "jsonpath={.status.phase}",
					"-n", namespace)
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("Succeeded"), "curl pod in wrong status")
			}
			Eventually(verifyCurlUp, 5*time.Minute).Should(Succeed())

			By("getting the metrics by checking curl-metrics logs")
			metricsOutput := getMetricsOutput()
			Expect(metricsOutput).To(ContainSubstring(
				"controller_runtime_reconcile_total",
			))
		})

		// +kubebuilder:scaffold:e2e-webhooks-checks
	})

	Context("Infrastructure", func() {
		It("should have all pods running", func() {
			for _, label := range []string{"nats", "runtime-operator", "runtime-gateway", "hostgroup"} {
				verifyPodReady := func(g Gomega) {
					cmd := exec.Command("kubectl", "wait", "--for=condition=Ready",
						"pod", "-l", fmt.Sprintf("wasmcloud.com/name=%s", label),
						"-n", namespace, "--timeout=10s")
					_, err := utils.Run(cmd)
					g.Expect(err).NotTo(HaveOccurred())
				}
				Eventually(verifyPodReady).WithTimeout(3 * time.Minute).Should(Succeed())
			}
		})
	})

	Context("Host Registration", func() {
		It("should register at least one host", func() {
			verifyHostRegistered := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "hosts.runtime.wasmcloud.dev",
					"-n", namespace, "-o", "jsonpath={.items}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).NotTo(Equal("[]"), "no hosts registered yet")
			}
			Eventually(verifyHostRegistered).WithTimeout(2 * time.Minute).Should(Succeed())
		})
	})

	Context("Workload Lifecycle", func() {
		const sampleDeployment = "config/samples/deployment.yaml"

		It("should deploy a workload and become ready", func() {
			verifyWorkloadDeploy(sampleDeployment)
		})

		It("should serve HTTP traffic through the gateway", func() {
			verifyHTTP := func(g Gomega) {
				cmd := exec.Command("curl", "-s", "-o", "/dev/null",
					"-w", "%{http_code}",
					"-H", "Host: hello.localhost.direct",
					"http://localhost:80")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("200"))
			}
			Eventually(verifyHTTP).WithTimeout(1 * time.Minute).Should(Succeed())
		})

		It("should clean up workload resources on delete", func() {
			By("deleting the WorkloadDeployment")
			cmd := exec.Command("kubectl", "delete", "workloaddeployment", "hello",
				"-n", namespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for Workload CRs to be cleaned up")
			verifyCleanup := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
					"-n", namespace, "-o", "jsonpath={.items}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("[]"))
			}
			Eventually(verifyCleanup).WithTimeout(1 * time.Minute).Should(Succeed())
		})

	})

	Context("Workload w/Service Lifecycle", func() {
		BeforeEach(func() {
			if !runtimeSupportsHostAliases {
				Skip("runtime does not support HostAliases, skipping EndpointSlice tests")
			}
		})

		const sampleDeployment = "config/samples/service_deployment.yaml"

		// Delete runtime-gateway Service and Deployment before tests in this context
		// to ensure we're testing the Service with EndpointSlices.
		It("should remove runtime-gateway", func() {
			cmd := exec.Command("kubectl", "delete", "deployment", "runtime-gateway",
				"-n", namespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to delete runtime-gateway deployment")

			cmd = exec.Command("kubectl", "delete", "service", "runtime-gateway",
				"-n", namespace)
			_, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(), "Failed to delete runtime-gateway service")
		})

		It("should deploy a workload and become ready", func() {
			verifyWorkloadDeploy(sampleDeployment)
		})

		It("should serve HTTP traffic through the gateway", func() {
			verifyHTTP := func(g Gomega) {
				cmd := exec.Command("curl", "-s", "-o", "/dev/null",
					"-w", "%{http_code}",
					"-H", "Host: hello-workload.default",
					"http://localhost:80")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("200"))
			}
			Eventually(verifyHTTP).WithTimeout(1 * time.Minute).Should(Succeed())
		})

		It("should clean up workload resources on delete", func() {
			By("deleting the WorkloadDeployment")
			cmd := exec.Command("kubectl", "delete", "workloaddeployment", "hello",
				"-n", namespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for Workload CRs to be cleaned up")
			verifyCleanup := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
					"-n", namespace, "-o", "jsonpath={.items}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("[]"))
			}
			Eventually(verifyCleanup).WithTimeout(1 * time.Minute).Should(Succeed())
		})
	})

	Context("Finalizer", func() {
		It("should terminate all hostgroup pods when scaled to zero to test finalizer", func() {
			By("scaling the hostgroup deployment to zero")
			cmd := exec.Command("kubectl", "scale", "deployment",
				"-l", "wasmcloud.com/name=hostgroup",
				"--replicas=0", "-n", namespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for all hostgroup pods to be removed")
			verifyNoPods := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "pods",
					"-l", "wasmcloud.com/name=hostgroup",
					"-n", namespace, "-o", "jsonpath={.items}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("[]"))
			}
			Eventually(verifyNoPods).WithTimeout(2 * time.Minute).Should(Succeed())
		})
	})
})

// verifyWorkloadDeploy applies a WorkloadDeployment manifest and verifies the
// deployment becomes ready, along with its ReplicaSet and Workload CRs.
func verifyWorkloadDeploy(sampleDeployment string) {
	By("applying the sample WorkloadDeployment")
	cmd := exec.Command("kubectl", "apply", "-n", namespace, "-f", sampleDeployment)
	_, err := utils.Run(cmd)
	Expect(err).NotTo(HaveOccurred())

	By("waiting for WorkloadDeployment to become Ready")
	verifyWorkloadReady := func(g Gomega) {
		cmd := exec.Command("kubectl", "get", "workloaddeployment", "hello",
			"-n", namespace,
			"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
		output, err := utils.Run(cmd)
		g.Expect(err).NotTo(HaveOccurred())
		g.Expect(output).To(Equal("True"))
	}
	Eventually(verifyWorkloadReady).WithTimeout(3 * time.Minute).Should(Succeed())

	By("verifying WorkloadReplicaSet was created")
	cmd = exec.Command("kubectl", "get", "workloadreplicasets.runtime.wasmcloud.dev",
		"-n", namespace, "-o", "jsonpath={.items}")
	output, err := utils.Run(cmd)
	Expect(err).NotTo(HaveOccurred())
	Expect(output).NotTo(Equal("[]"))

	By("verifying Workload CR was created")
	cmd = exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
		"-n", namespace, "-o", "jsonpath={.items}")
	output, err = utils.Run(cmd)
	Expect(err).NotTo(HaveOccurred())
	Expect(output).NotTo(Equal("[]"))
}

// serviceAccountToken returns a token for the specified service account in the given namespace.
// It uses the Kubernetes TokenRequest API to generate a token by directly sending a request
// and parsing the resulting token from the API response.
func serviceAccountToken() (string, error) {
	const tokenRequestRawString = `{
		"apiVersion": "authentication.k8s.io/v1",
		"kind": "TokenRequest"
	}`

	// Temporary file to store the token request
	secretName := fmt.Sprintf("%s-token-request", serviceAccountName)
	tokenRequestFile := filepath.Join("/tmp", secretName)
	err := os.WriteFile(tokenRequestFile, []byte(tokenRequestRawString), os.FileMode(0o644))
	if err != nil {
		return "", err
	}

	var out string
	verifyTokenCreation := func(g Gomega) {
		// Execute kubectl command to create the token
		cmd := exec.Command("kubectl", "create", "--raw", fmt.Sprintf(
			"/api/v1/namespaces/%s/serviceaccounts/%s/token",
			namespace,
			serviceAccountName,
		), "-f", tokenRequestFile)

		output, err := cmd.CombinedOutput()
		g.Expect(err).NotTo(HaveOccurred())

		// Parse the JSON output to extract the token
		var token tokenRequest
		err = json.Unmarshal(output, &token)
		g.Expect(err).NotTo(HaveOccurred())

		out = token.Status.Token
	}
	Eventually(verifyTokenCreation).Should(Succeed())

	return out, err
}

// getMetricsOutput retrieves and returns the logs from the curl pod used to access the metrics endpoint.
func getMetricsOutput() string {
	By("getting the curl-metrics logs")
	cmd := exec.Command("kubectl", "logs", "curl-metrics", "-n", namespace)
	metricsOutput, err := utils.Run(cmd)
	Expect(err).NotTo(HaveOccurred(), "Failed to retrieve logs from curl pod")
	Expect(metricsOutput).To(ContainSubstring("< HTTP/1.1 200 OK"))
	return metricsOutput
}

// tokenRequest is a simplified representation of the Kubernetes TokenRequest API response,
// containing only the token field that we need to extract.
type tokenRequest struct {
	Status struct {
		Token string `json:"token"`
	} `json:"status"`
}
