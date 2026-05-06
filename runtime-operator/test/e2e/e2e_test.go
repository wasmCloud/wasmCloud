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

	// Tenant-namespace scenario: exercises `helm upgrade` on top of the
	// already-installed release, adding a hostGroup whose pods land in a
	// separate tenant namespace, and verifies that:
	//   * the chart auto-renders per-namespace TLS Secrets, ServiceAccount,
	//     and Pod RBAC for the tenant namespace,
	//   * the operator's `-host-namespaces` flag is auto-derived from
	//     `runtime.hostGroups[].namespace` via the
	//     `runtime-operator.hostNamespaces` chart helper,
	//   * the host pod's heartbeat carries the tenant namespace as its
	//     `environment` field, which the operator records on the
	//     resulting `Host` CRD,
	//   * a WorkloadDeployment whose `spec.template.spec.environment`
	//     matches the tenant namespace schedules onto the tenant hosts,
	//     and the resulting `Workload.status.environment` reflects the
	//     same value.
	//
	// Cleanup: AfterAll deletes the WorkloadDeployment created here;
	// AfterSuite's `helm delete` removes the helm-managed resources in
	// the tenant namespace. The tenant namespace itself is left behind
	// for inspection — it is removed when the kind cluster is destroyed.
	Context("Tenant Namespace via Helm Upgrade", Ordered, func() {
		const (
			tenantNamespace    = "namespace-a"
			tenantHostGroup    = "tenant"
			tenantWorkloadName = "hello-tenant"
		)

		var workloadFile string

		AfterAll(func() {
			By("deleting the tenant WorkloadDeployment")
			cmd := exec.Command("kubectl", "delete", "workloaddeployment",
				tenantWorkloadName, "-n", tenantNamespace, "--ignore-not-found")
			_, _ = utils.Run(cmd)

			if workloadFile != "" {
				_ = os.Remove(workloadFile)
			}
		})

		It("creates the tenant namespace", func() {
			cmd := exec.Command("kubectl", "create", "namespace", tenantNamespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(),
				"failed to create tenant namespace %q", tenantNamespace)
		})

		It("performs a helm upgrade adding a hostGroup in the tenant namespace", func() {
			// Append a second hostGroup pinned to the tenant namespace.
			// We deliberately do NOT set `operator.hostNamespaces` here
			// — the chart's `runtime-operator.hostNamespaces` helper
			// should auto-derive it from the hostGroup's namespace
			// override, and we assert that below.
			sets := append(buildBaseHelmSets(),
				fmt.Sprintf("runtime.hostGroups[1].name=%s", tenantHostGroup),
				fmt.Sprintf("runtime.hostGroups[1].namespace=%s", tenantNamespace),
				"runtime.hostGroups[1].replicas=1",
				"runtime.hostGroups[1].service.type=ClusterIP",
				"runtime.hostGroups[1].http.enabled=true",
				"runtime.hostGroups[1].http.port=80",
				"runtime.hostGroups[1].webgpu.enabled=false",
				"runtime.hostGroups[1].resources.requests.memory=64Mi",
				"runtime.hostGroups[1].resources.requests.cpu=250m",
				"runtime.hostGroups[1].resources.limits.memory=512Mi",
				"runtime.hostGroups[1].resources.limits.cpu=500m",
				fmt.Sprintf("runtime.hostGroups[1].logLevel=%s", runtimeLogLevel),
			)

			helmArgs := make([]string, 0, 5+2*len(sets)+4)
			helmArgs = append(helmArgs, "upgrade", "--install", "-n", namespace)
			for _, s := range sets {
				helmArgs = append(helmArgs, "--set", s)
			}
			helmArgs = append(helmArgs, "--wait", "--timeout=5m",
				"operator-e2e", helmChartPath)

			cmd := exec.Command("helm", helmArgs...)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(),
				"helm upgrade with tenant hostGroup failed")
		})

		It("renders per-namespace TLS Secrets and Pod RBAC in the tenant namespace", func() {
			// The chart's certificates.yaml replicates runtime-tls and
			// data-tls into every host-pod namespace; without these the
			// host pod can't mount its volumes and stays in
			// ContainerCreating.
			for _, secret := range []string{"wasmcloud-runtime-tls", "wasmcloud-data-tls"} {
				cmd := exec.Command("kubectl", "get", "secret", secret,
					"-n", tenantNamespace)
				_, err := utils.Run(cmd)
				Expect(err).NotTo(HaveOccurred(),
					"expected Secret %s in %s", secret, tenantNamespace)
			}

			// host-pod-role.yaml renders one Pod-only Role + RoleBinding
			// per non-release host namespace.
			for _, kind := range []string{"role", "rolebinding"} {
				cmd := exec.Command("kubectl", "get", kind,
					"operator-e2e-runtime-operator-host-pod",
					"-n", tenantNamespace)
				_, err := utils.Run(cmd)
				Expect(err).NotTo(HaveOccurred(),
					"expected %s operator-e2e-runtime-operator-host-pod in %s",
					kind, tenantNamespace)
			}
		})

		It("configures the operator with -host-namespaces including the tenant namespace", func() {
			// The chart's runtime-operator.hostNamespaces helper unions
			// operator.hostNamespaces with runtime.hostGroups[].namespace,
			// so adding a tenant-namespaced hostGroup alone should be
			// enough to flip on the per-namespace Pod cache + RBAC.
			verifyArgs := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "deployment",
					"runtime-operator", "-n", namespace,
					"-o", "jsonpath={.spec.template.spec.containers[0].args}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(ContainSubstring(
					fmt.Sprintf("-host-namespaces=%s", tenantNamespace)),
					"operator should pass -host-namespaces=%s; got: %s",
					tenantNamespace, output)
			}
			Eventually(verifyArgs).WithTimeout(2 * time.Minute).Should(Succeed())
		})

		It("brings up a host pod in the tenant namespace", func() {
			verifyTenantHostReady := func(g Gomega) {
				cmd := exec.Command("kubectl", "wait", "--for=condition=Ready",
					"pod", "-l", fmt.Sprintf("wasmcloud.com/hostgroup=%s", tenantHostGroup),
					"-n", tenantNamespace, "--timeout=10s")
				_, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
			}
			Eventually(verifyTenantHostReady).WithTimeout(3 * time.Minute).Should(Succeed())
		})

		It("registers a Host CRD with environment matching the tenant namespace", func() {
			// Hosts always live in the operator's own namespace (per the
			// namespaced-but-centrally-stored design); the heartbeat's
			// `environment` field — sourced from the host pod's downward
			// API namespace — is recorded verbatim on Host.spec.environment.
			verifyHostEnv := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "hosts.runtime.wasmcloud.dev",
					"-n", namespace,
					"-l", fmt.Sprintf("hostgroup=%s", tenantHostGroup),
					"-o", "jsonpath={.items[*].environment}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(ContainSubstring(tenantNamespace),
					"expected at least one Host with environment=%s; got %q",
					tenantNamespace, output)
			}
			Eventually(verifyHostEnv).WithTimeout(2 * time.Minute).Should(Succeed())
		})

		It("schedules a WorkloadDeployment with matching environment onto the tenant host", func() {
			manifest := fmt.Sprintf(`apiVersion: runtime.wasmcloud.dev/v1alpha1
kind: WorkloadDeployment
metadata:
  name: %s
  namespace: %s
spec:
  replicas: 1
  template:
    spec:
      environment: %s
      hostSelector:
        hostgroup: %s
      hostInterfaces:
        - namespace: wasi
          package: http
          interfaces:
            - incoming-handler
          config:
            host: hello.localhost.direct
      components:
        - name: hello-world
          image: ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0
`, tenantWorkloadName, tenantNamespace, tenantNamespace, tenantHostGroup)

			f, err := os.CreateTemp("", "tenant-workload-*.yaml")
			Expect(err).NotTo(HaveOccurred())
			workloadFile = f.Name()
			_, err = f.WriteString(manifest)
			Expect(err).NotTo(HaveOccurred())
			Expect(f.Close()).To(Succeed())

			By("applying the tenant WorkloadDeployment")
			cmd := exec.Command("kubectl", "apply", "-f", workloadFile)
			_, err = utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for the WorkloadDeployment to become Ready")
			verifyReady := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "workloaddeployment",
					tenantWorkloadName, "-n", tenantNamespace,
					"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("True"))
			}
			Eventually(verifyReady).WithTimeout(3 * time.Minute).Should(Succeed())

			By("verifying the resulting Workload Status.Environment is the tenant namespace")
			verifyWorkloadEnv := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
					"-n", tenantNamespace,
					"-o", "jsonpath={.items[*].status.environment}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(ContainSubstring(tenantNamespace),
					"expected workload Status.Environment=%s; got %q",
					tenantNamespace, output)
			}
			Eventually(verifyWorkloadEnv).WithTimeout(2 * time.Minute).Should(Succeed())
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
