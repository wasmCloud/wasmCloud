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
	"strings"
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
		const (
			sampleDeployment = "config/samples/deployment.yaml"
			deploymentName   = "hello"
		)

		It("should deploy a workload and become ready", func() {
			verifyWorkloadDeploy(sampleDeployment, deploymentName, namespace)
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
			cmd := exec.Command("kubectl", "delete", "workloaddeployment", deploymentName,
				"-n", namespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for Workload CRs to be cleaned up")
			verifyCleanup := func(g Gomega) {
				expectNoTestWorkloads(g, namespace)
			}
			Eventually(verifyCleanup).WithTimeout(1 * time.Minute).Should(Succeed())
		})

	})

	Context("Workload w/Service Lifecycle", func() {
		const (
			sampleDeployment = "config/samples/service_deployment.yaml"
			deploymentName   = "hello-workload"
		)

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
			verifyWorkloadDeploy(sampleDeployment, deploymentName, namespace)
		})

		// The route controller stamps an EndpointSlice with a Service
		// ownerRef + blockOwnerDeletion=true once a referencing Workload is
		// Ready. EndpointSlice creation is independent of whether the wash
		// runtime supports HostAliases, and missing RBAC on
		// services/finalizers (under OwnerReferencesPermissionEnforcement)
		// surfaces only via this assertion. Workload readiness reports
		// success even when the route controller's Create is denied.
		It("should create an EndpointSlice for the workload service", func() {
			verifyEndpointSlice := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "endpointslices",
					"-n", namespace,
					"-l", "kubernetes.io/service-name=hello-workload,wasmcloud.dev/route-manager=true",
					"-o", "jsonpath={.items}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).NotTo(Equal("[]"), "no operator-managed EndpointSlice for hello-workload")
			}
			Eventually(verifyEndpointSlice).WithTimeout(30 * time.Second).Should(Succeed())
		})

		// Catch admission denials the operator has tried-and-logged but that
		// don't surface via a specific resource assertion. The Kubernetes API
		// formats all RBAC rejections as `<resource>.<group> "<name>" is
		// forbidden: <reason>`, so a substring grep on the operator log is a
		// reliable generic signal.
		It("should not have logged any forbidden errors", func() {
			cmd := exec.Command("kubectl", "logs",
				"-n", namespace,
				"-l", "wasmcloud.com/name=runtime-operator",
				"--tail=500")
			output, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())
			Expect(output).NotTo(ContainSubstring("is forbidden"),
				"operator log contains an admission-denied error — likely a missing RBAC rule")
		})

		It("should serve HTTP traffic through the gateway", func() {
			if !runtimeSupportsHostAliases {
				Skip("runtime does not support HostAliases, skipping HTTP traffic test")
			}
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
			// Delete via the manifest so the hello-workload Service (which
			// claimed nodePort 30950 once the gateway was removed) is also
			// torn down — otherwise the next context's `helm upgrade`,
			// which re-creates the gateway Service on the same port, fails
			// with "provided port is already allocated".
			By("deleting the sample manifest")
			cmd := exec.Command("kubectl", "delete", "-f", sampleDeployment,
				"-n", namespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for Workload CRs to be cleaned up")
			verifyCleanup := func(g Gomega) {
				expectNoTestWorkloads(g, namespace)
			}
			Eventually(verifyCleanup).WithTimeout(1 * time.Minute).Should(Succeed())
		})
	})

	Context("Finalizer", func() {
		It("should terminate the default hostgroup pods when scaled to zero to test finalizer", func() {
			// Scale only the `default` hostgroup, not every hostgroup. On the
			// all-features leg the `registry` hostgroup runs the in-cluster
			// oci-registry, whose in-memory contents the later Tenant/Scoped
			// specs (and the randomized messaging/implements specs) still pull.
			By("scaling the default hostgroup deployment to zero")
			cmd := exec.Command("kubectl", "scale", "deployment",
				"-l", "wasmcloud.com/hostgroup=default",
				"--replicas=0", "-n", namespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for the default hostgroup pods to be removed")
			verifyNoPods := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "pods",
					"-l", "wasmcloud.com/hostgroup=default",
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
			// Append a hostGroup pinned to the tenant namespace, at the index
			// after buildBaseHelmSets's groups (extraHostGroupIndex: [2] when the
			// registry occupies [1], else [1]). When the registry flow is active
			// this host runs the http fixture pulled from the in-cluster registry,
			// so it's made insecure to match. We deliberately do NOT set
			// `operator.hostNamespaces` here — the chart's
			// `runtime-operator.hostNamespaces` helper should auto-derive it from
			// the hostGroup's namespace override, and we assert that below.
			hg := fmt.Sprintf("runtime.hostGroups[%d]", extraHostGroupIndex())
			sets := append(buildBaseHelmSets(),
				fmt.Sprintf("%s.name=%s", hg, tenantHostGroup),
				fmt.Sprintf("%s.namespace=%s", hg, tenantNamespace),
				fmt.Sprintf("%s.replicas=1", hg),
				fmt.Sprintf("%s.service.type=ClusterIP", hg),
				fmt.Sprintf("%s.http.enabled=true", hg),
				fmt.Sprintf("%s.http.port=80", hg),
				fmt.Sprintf("%s.webgpu.enabled=false", hg),
				fmt.Sprintf("%s.resources.requests.memory=64Mi", hg),
				fmt.Sprintf("%s.resources.requests.cpu=250m", hg),
				fmt.Sprintf("%s.resources.limits.memory=512Mi", hg),
				fmt.Sprintf("%s.resources.limits.cpu=500m", hg),
				fmt.Sprintf("%s.logLevel=%s", hg, runtimeLogLevel),
			)
			if inClusterRegistry {
				sets = append(sets, fmt.Sprintf("%s.extraArgs[0]=--allow-insecure-registries", hg))
			}

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
          image: %s
`, tenantWorkloadName, tenantNamespace, tenantNamespace, tenantHostGroup, httpWorkloadImage())

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

	// Scoped-install scenario: switches the operator release into
	// watchNamespaces mode via `helm upgrade --reset-values`, then verifies:
	//   * the workload-side ClusterRole/ClusterRoleBinding are GC'd by Helm;
	//   * per-namespace `workload-crd`, `workload-namespace`, and
	//     `endpointslice` Roles + RoleBindings exist in the watched namespace
	//     and are bound to the operator SA;
	//   * a WorkloadDeployment in the watched namespace reconciles to Ready
	//     — covers create/update of the CRDs, their `/status`, and the
	//     `<owner>/finalizers` stamp the GC admission plugin requires on
	//     child ownerRefs;
	//   * a WorkloadDeployment in an unwatched namespace is NOT reconciled
	//     (Consistently over 30s — proves informer scoping holds);
	//   * the operator log has no `is forbidden` admission denial — catch-all
	//     for any controller path that missed a per-namespace permission;
	//   * deleting the scoped WorkloadDeployment walks the full delete
	//     reconcile cleanly under the per-namespace Role.
	//
	// Cleanup: AfterAll deletes both WorkloadDeployments, restores the
	// release to watch-all mode (so later specs that assume it aren't broken
	// by the scope switch), and tears down the two scoped namespaces.
	// AfterSuite's `helm delete` removes the helm-managed resources.
	Context("Scoped Install via watchNamespaces", Ordered, func() {
		const (
			watchedNamespace     = "scoped-watched"
			unwatchedNamespace   = "scoped-unwatched"
			scopedHostGroup      = "scoped-host"
			scopedWorkloadName   = "hello-scoped"
			unscopedWorkloadName = "hello-unscoped"
			// chartFullname is the value of the `runtime-operator.fullname`
			// helper for this suite's release. The chart joins release name
			// and chart name unless one already contains the other; the e2e
			// release is `operator-e2e` and the chart is `runtime-operator`.
			chartFullname = "operator-e2e-runtime-operator"
		)

		var (
			scopedWorkloadFile   string
			unscopedWorkloadFile string
		)

		// Writes a hello-world WorkloadDeployment manifest into a temp file
		// and returns the path. environment is pinned to the namespace so
		// the workload schedules onto a host whose heartbeat carries the
		// same value (see Tenant block for the same convention).
		writeWorkloadManifest := func(name, ns, hostgroup string) string {
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
          image: %s
`, name, ns, ns, hostgroup, httpWorkloadImage())

			f, err := os.CreateTemp("", "scoped-workload-*.yaml")
			Expect(err).NotTo(HaveOccurred())
			_, err = f.WriteString(manifest)
			Expect(err).NotTo(HaveOccurred())
			Expect(f.Close()).To(Succeed())
			return f.Name()
		}

		// helm upgrade --install --reset-values on the suite release with
		// buildBaseHelmSets plus extraSets. --reset-values gives a clean
		// slate from chart defaults so the operator config is determined
		// solely by what we pass, discarding any prior upgrade's --sets.
		helmUpgrade := func(extraSets []string, failMsg string) {
			sets := append(buildBaseHelmSets(), extraSets...)
			args := make([]string, 0, 6+2*len(sets)+4)
			args = append(args, "upgrade", "--install", "--reset-values",
				"-n", namespace)
			for _, s := range sets {
				args = append(args, "--set", s)
			}
			args = append(args, "--wait", "--timeout=5m",
				"operator-e2e", helmChartPath)
			_, err := utils.Run(exec.Command("helm", args...))
			Expect(err).NotTo(HaveOccurred(), failMsg)
		}

		AfterAll(func() {
			// Cleanup: the It blocks below delete both
			// WorkloadDeployments, but if any It failed early the resource
			// may still exist. --timeout=30s bounds the wait in case a
			// finalizer can't clear (e.g. if a host pod is gone), so a
			// stuck delete doesn't hang the whole suite.
			By("deleting WorkloadDeployments created by the scoped block")
			if scopedWorkloadFile != "" {
				cmd := exec.Command("kubectl", "delete", "workloaddeployment",
					scopedWorkloadName, "-n", watchedNamespace,
					"--ignore-not-found", "--timeout=30s")
				_, _ = utils.Run(cmd)
				_ = os.Remove(scopedWorkloadFile)
			}
			if unscopedWorkloadFile != "" {
				cmd := exec.Command("kubectl", "delete", "workloaddeployment",
					unscopedWorkloadName, "-n", unwatchedNamespace,
					"--ignore-not-found", "--timeout=30s")
				_, _ = utils.Run(cmd)
				_ = os.Remove(unscopedWorkloadFile)
			}

			// Restore watch-all before leaving this block. The scoped
			// upgrade switched the *whole* release into watchNamespaces
			// mode; other top-level specs (e.g. Messaging) run against the
			// default namespace and assume watch-all, and Ginkgo may order
			// them after this block. AfterSuite only deletes the release, so
			// without this restore those specs would race an operator that
			// no longer watches their namespace. Do this while the watched
			// namespace still exists so Helm can cleanly remove its per-ns
			// Roles, then tear the namespaces down.
			By("restoring the release to watch-all mode")
			helmUpgrade(nil, "helm upgrade restoring watch-all mode failed")

			By("deleting scoped namespaces")
			// AfterSuite's `helm delete` removes only helm-tracked
			// resources; the namespaces themselves were created with
			// kubectl, so we own their teardown here. Best-effort with a
			// timeout so a stuck finalizer doesn't hang the suite.
			for _, ns := range []string{watchedNamespace, unwatchedNamespace} {
				cmd := exec.Command("kubectl", "delete", "namespace", ns,
					"--ignore-not-found", "--timeout=60s")
				_, _ = utils.Run(cmd)
			}
		})

		It("creates the watched and unwatched namespaces", func() {
			for _, ns := range []string{watchedNamespace, unwatchedNamespace} {
				cmd := exec.Command("kubectl", "create", "namespace", ns)
				_, err := utils.Run(cmd)
				Expect(err).NotTo(HaveOccurred(),
					"failed to create namespace %q", ns)
			}
		})

		It("helm upgrades the release into scoped mode", func() {
			// Run a host inside the watched namespace so a workload applied there
			// has something to schedule onto, at the index after buildBaseHelmSets's
			// groups (extraHostGroupIndex: [2] when the registry occupies [1], else
			// [1]). When the registry flow is active it runs the http fixture from
			// the in-cluster registry, so it's made insecure to match. The chart's
			// `runtime-operator.hostNamespaces` helper auto-derives -host-namespaces
			// from runtime.hostGroups[].namespace, so we don't set
			// operator.hostNamespaces directly.
			hg := fmt.Sprintf("runtime.hostGroups[%d]", extraHostGroupIndex())
			scopedSets := []string{
				fmt.Sprintf("operator.watchNamespaces[0]=%s", watchedNamespace),
				fmt.Sprintf("%s.name=%s", hg, scopedHostGroup),
				fmt.Sprintf("%s.namespace=%s", hg, watchedNamespace),
				fmt.Sprintf("%s.replicas=1", hg),
				fmt.Sprintf("%s.service.type=ClusterIP", hg),
				fmt.Sprintf("%s.http.enabled=true", hg),
				fmt.Sprintf("%s.http.port=80", hg),
				fmt.Sprintf("%s.webgpu.enabled=false", hg),
				fmt.Sprintf("%s.resources.requests.memory=64Mi", hg),
				fmt.Sprintf("%s.resources.requests.cpu=250m", hg),
				fmt.Sprintf("%s.resources.limits.memory=512Mi", hg),
				fmt.Sprintf("%s.resources.limits.cpu=500m", hg),
				fmt.Sprintf("%s.logLevel=%s", hg, runtimeLogLevel),
			}
			if inClusterRegistry {
				scopedSets = append(scopedSets, fmt.Sprintf("%s.extraArgs[0]=--allow-insecure-registries", hg))
			}
			helmUpgrade(scopedSets, "helm upgrade into scoped mode failed")
		})

		It("removes the cluster-scoped operator RBAC", func() {
			// The watch-all clusterrole.yaml / clusterrolebinding.yaml are
			// now gated on `not .Values.operator.watchNamespaces`, so Helm
			// must reconcile them away on the upgrade. Their lingering
			// presence would silently grant cluster-wide CRD verbs to the
			// operator SA and defeat the whole point of watchNamespaces.
			for _, kind := range []string{"clusterrole", "clusterrolebinding"} {
				cmd := exec.Command("kubectl", "get", kind, chartFullname,
					"--ignore-not-found", "-o", "name")
				output, err := utils.Run(cmd)
				Expect(err).NotTo(HaveOccurred())
				Expect(output).To(BeEmpty(),
					"%s %s should be absent in scoped mode", kind, chartFullname)
			}
		})

		It("creates per-namespace workload Roles and RoleBindings", func() {
			roleNames := []string{
				chartFullname + "-workload-crd",
				chartFullname + "-workload-namespace",
				chartFullname + "-endpointslice",
			}
			for _, kind := range []string{"role", "rolebinding"} {
				for _, name := range roleNames {
					cmd := exec.Command("kubectl", "get", kind, name,
						"-n", watchedNamespace)
					_, err := utils.Run(cmd)
					Expect(err).NotTo(HaveOccurred(),
						"expected %s %s in %s", kind, name, watchedNamespace)
				}
			}
		})

		It("brings up a host pod in the watched namespace", func() {
			verifyHostReady := func(g Gomega) {
				cmd := exec.Command("kubectl", "wait", "--for=condition=Ready",
					"pod", "-l", fmt.Sprintf("wasmcloud.com/hostgroup=%s", scopedHostGroup),
					"-n", watchedNamespace, "--timeout=10s")
				_, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
			}
			Eventually(verifyHostReady).WithTimeout(3 * time.Minute).Should(Succeed())
		})

		It("deploys a workload in the watched namespace", func() {
			scopedWorkloadFile = writeWorkloadManifest(
				scopedWorkloadName, watchedNamespace, scopedHostGroup)
			verifyWorkloadDeploy(scopedWorkloadFile, scopedWorkloadName, watchedNamespace)
		})

		It("does not reconcile a workload in an unwatched namespace", func() {
			// `cacheOpts.DefaultNamespaces` constrains the workload informer
			// to watchNamespaces, so a WorkloadDeployment applied outside
			// that set must remain completely untouched. We Consistently
			// assert across 30s that the controller never:
			//   * created a child WorkloadReplicaSet or Workload CR,
			//   * added a finalizer to the WorkloadDeployment,
			//   * wrote any status conditions.
			// Together these cover every observable side effect of a
			// reconcile pass, including the earliest (finalizer stamp).
			unscopedWorkloadFile = writeWorkloadManifest(
				unscopedWorkloadName, unwatchedNamespace, scopedHostGroup)

			By("applying the unwatched-namespace WorkloadDeployment")
			cmd := exec.Command("kubectl", "apply", "-f", unscopedWorkloadFile)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			// jsonpath emits an empty string when the resolved path is nil
			// and `[]` when it resolves to an empty list. Accept either as
			// "controller never touched this field".
			emptyOrNil := Or(Equal(""), Equal("[]"))

			verifyUnreconciled := func(g Gomega) {
				for _, resource := range []string{
					"workloads.runtime.wasmcloud.dev",
					"workloadreplicasets.runtime.wasmcloud.dev",
				} {
					cmd := exec.Command("kubectl", "get", resource,
						"-n", unwatchedNamespace, "-o", "jsonpath={.items}")
					output, err := utils.Run(cmd)
					g.Expect(err).NotTo(HaveOccurred())
					g.Expect(output).To(Equal("[]"),
						"operator should not have created any %s in an unwatched namespace",
						resource)
				}

				for _, field := range []string{
					".status.conditions",
					".metadata.finalizers",
				} {
					cmd := exec.Command("kubectl", "get", "workloaddeployment",
						unscopedWorkloadName, "-n", unwatchedNamespace,
						"-o", fmt.Sprintf("jsonpath={%s}", field))
					output, err := utils.Run(cmd)
					g.Expect(err).NotTo(HaveOccurred())
					g.Expect(output).To(emptyOrNil,
						"WorkloadDeployment in unwatched namespace should have no %s; got %q",
						field, output)
				}
			}
			Consistently(verifyUnreconciled).WithTimeout(30 * time.Second).Should(Succeed())
		})

		// The Kubernetes apiserver formats every RBAC rejection as
		// `<resource>.<group> "<name>" is forbidden: <reason>`, so a
		// substring scan of the operator's log catches any controller path
		// that hit a permission the per-namespace Role didn't grant.
		It("should not have logged any forbidden errors", func() {
			cmd := exec.Command("kubectl", "logs",
				"-n", namespace,
				"-l", "wasmcloud.com/name=runtime-operator",
				"--tail=500")
			output, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())
			Expect(output).NotTo(ContainSubstring("is forbidden"),
				"operator log contains an admission-denied error in scoped mode — likely a missing per-namespace RBAC rule")
		})

		It("cleans up the scoped WorkloadDeployment on delete", func() {
			// Run this before the hostgroup scale-to-zero below: with the
			// host still up, the operator can clear its finalizer cleanly,
			// which proves the per-namespace workload-crd Role grants
			// enough to walk the full delete path (status update on
			// children + finalizer removal on the parent). Running it now
			// also keeps AfterAll's safety-net delete fast.
			By("deleting the scoped WorkloadDeployment")
			cmd := exec.Command("kubectl", "delete", "workloaddeployment",
				scopedWorkloadName, "-n", watchedNamespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			By("waiting for child Workload CRs to be cleaned up")
			verifyCleanup := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
					"-n", watchedNamespace, "-o", "jsonpath={.items}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("[]"))
			}
			Eventually(verifyCleanup).WithTimeout(1 * time.Minute).Should(Succeed())
		})

		It("cleans up the unwatched WorkloadDeployment on delete", func() {
			// The unwatched resource was never reconciled, so no finalizer
			// was added — delete should return immediately. This both
			// verifies our negative-reconcile assertion (no finalizer means
			// delete completes synchronously) and keeps AfterAll quick.
			By("deleting the unwatched WorkloadDeployment")
			cmd := exec.Command("kubectl", "delete", "workloaddeployment",
				unscopedWorkloadName, "-n", unwatchedNamespace, "--timeout=30s")
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred(),
				"delete should be instant since the controller never added a finalizer; "+
					"a timeout here implies the operator did reconcile the unwatched workload")
		})

		It("scales the watched hostgroup to zero", func() {
			// Hostgroup teardown sanity check. The workload finalizer path
			// was exercised by the preceding workload-delete It, so this
			// runs with no WorkloadDeployment referencing the hostgroup —
			// any slow cleanup here is unambiguously a hostgroup-side
			// signal, not a confused workload finalizer.
			By("scaling hostgroup deployments in the watched namespace to zero")
			cmd := exec.Command("kubectl", "scale", "deployment",
				"-l", fmt.Sprintf("wasmcloud.com/hostgroup=%s", scopedHostGroup),
				"--replicas=0", "-n", watchedNamespace)
			_, err := utils.Run(cmd)
			Expect(err).NotTo(HaveOccurred())

			verifyNoPods := func(g Gomega) {
				cmd := exec.Command("kubectl", "get", "pods",
					"-l", fmt.Sprintf("wasmcloud.com/hostgroup=%s", scopedHostGroup),
					"-n", watchedNamespace, "-o", "jsonpath={.items}")
				output, err := utils.Run(cmd)
				g.Expect(err).NotTo(HaveOccurred())
				g.Expect(output).To(Equal("[]"))
			}
			Eventually(verifyNoPods).WithTimeout(2 * time.Minute).Should(Succeed())
		})
	})
})

// httpHelloWorldImage is the published component the http-serving sample
// manifests (config/samples) reference. On the all-features leg the suite serves
// an equivalent in-tree fixture (http-handler-p2) from the in-cluster registry
// instead; other legs keep pulling this published ref.
const httpHelloWorldImage = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0"

// httpWorkloadImage is the image the http-serving specs deploy: the in-cluster
// http-handler-p2 fixture on the all-features leg, else the published
// http-hello-world component.
func httpWorkloadImage() string {
	if inClusterRegistry {
		return registryRef("http-handler-p2")
	}
	return httpHelloWorldImage
}

// rewriteWorkloadImages swaps the published http image in a sample manifest for
// its in-cluster registry equivalent when the registry flow is active; a no-op
// otherwise (the release/canary legs deploy the published component as-is).
func rewriteWorkloadImages(manifest string) string {
	if !inClusterRegistry {
		return manifest
	}
	return strings.ReplaceAll(manifest, httpHelloWorldImage, registryRef("http-handler-p2"))
}

// expectNoTestWorkloads asserts that no Workload CRs remain in ns once the
// in-cluster oci-registry infrastructure workload is excluded. The registry runs
// as a long-lived Workload in this namespace for the whole suite (serving every
// fixture image), so a bare "the items list is empty" check no longer holds on
// the legs that run it. When the registry flow is off, nothing is filtered and
// this is equivalent to asserting the list is empty.
func expectNoTestWorkloads(g Gomega, ns string) {
	cmd := exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
		"-n", ns, "-o", "jsonpath={.items[*].metadata.name}")
	output, err := utils.Run(cmd)
	g.Expect(err).NotTo(HaveOccurred())
	var remaining []string
	for _, name := range strings.Fields(output) {
		if strings.HasPrefix(name, "oci-registry") {
			continue
		}
		remaining = append(remaining, name)
	}
	g.Expect(remaining).To(BeEmpty(), "test Workload CRs were not cleaned up")
}

// verifyWorkloadDeploy applies a WorkloadDeployment manifest and verifies the
// deployment becomes ready, along with its ReplicaSet and Workload CRs.
// deploymentName is the metadata.name of the WorkloadDeployment in the sample
// manifest — needed so the Ready check targets the right CR. ns is the
// namespace to apply into and to query for child CRs; the manifest is applied
// with `-n ns` regardless of any metadata.namespace it carries.
func verifyWorkloadDeploy(sampleDeployment, deploymentName, ns string) {
	By("applying the sample WorkloadDeployment")
	// Rewrite any published workload image to its in-cluster registry
	// equivalent so the (insecure) hostgroups pull from the registry rather
	// than ghcr. No-op for manifests already built with registryRef.
	manifest, err := os.ReadFile(sampleDeployment)
	Expect(err).NotTo(HaveOccurred())
	cmd := exec.Command("kubectl", "apply", "-n", ns, "-f", "-")
	cmd.Stdin = strings.NewReader(rewriteWorkloadImages(string(manifest)))
	_, err = utils.Run(cmd)
	Expect(err).NotTo(HaveOccurred())

	By("waiting for WorkloadDeployment to become Ready")
	verifyWorkloadReady := func(g Gomega) {
		cmd := exec.Command("kubectl", "get", "workloaddeployment", deploymentName,
			"-n", ns,
			"-o", "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}")
		output, err := utils.Run(cmd)
		g.Expect(err).NotTo(HaveOccurred())
		g.Expect(output).To(Equal("True"))
	}
	Eventually(verifyWorkloadReady).WithTimeout(3 * time.Minute).Should(Succeed())

	By("verifying WorkloadReplicaSet was created")
	cmd = exec.Command("kubectl", "get", "workloadreplicasets.runtime.wasmcloud.dev",
		"-n", ns, "-o", "jsonpath={.items}")
	output, err := utils.Run(cmd)
	Expect(err).NotTo(HaveOccurred())
	Expect(output).NotTo(Equal("[]"))

	By("verifying Workload CR was created")
	cmd = exec.Command("kubectl", "get", "workloads.runtime.wasmcloud.dev",
		"-n", ns, "-o", "jsonpath={.items}")
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
