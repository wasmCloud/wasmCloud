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
	"testing"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

const envBoolTrue = "true"

var (
	// Optional Environment Variables:
	// - PROMETHEUS_INSTALL_SKIP=true: Skips Prometheus Operator installation during test setup (default: true).
	// - CERT_MANAGER_INSTALL_SKIP=true: Skips CertManager installation during test setup (default: true).
	skipPrometheusInstall  = os.Getenv("PROMETHEUS_INSTALL_SKIP") != "false"
	skipCertManagerInstall = os.Getenv("CERT_MANAGER_INSTALL_SKIP") != "false"
	// isPrometheusOperatorAlreadyInstalled will be set true when prometheus CRDs be found on the cluster
	isPrometheusOperatorAlreadyInstalled = false
	// isCertManagerAlreadyInstalled will be set true when CertManager CRDs be found on the cluster
	isCertManagerAlreadyInstalled = false

	// skipImageBuild skips the docker build step (set SKIP_IMAGE_BUILD=true when image is pre-built)
	skipImageBuild = os.Getenv("SKIP_IMAGE_BUILD") == envBoolTrue

	// operatorImageRepo and operatorImageTag are used for Helm --set overrides
	operatorImageRepo = "localhost/runtime-operator"
	operatorImageTag  = "e2e"
	// projectImage is the full image name built and loaded into Kind
	projectImage = fmt.Sprintf("%s:%s", operatorImageRepo, operatorImageTag)

	// runtimeImageRepo / runtimeImageTag identify the wash-runtime (host)
	// image. Set BUILD_RUNTIME_IMAGE=true to build from the local tree (the
	// only way the host pod actually exercises the code under test); leave
	// unset to use the published canary tag, which is faster but means the
	// e2e is testing whatever upstream shipped, not your branch. Set
	// SKIP_RUNTIME_BUILD=true alongside BUILD_RUNTIME_IMAGE=true to reuse a
	// previously-built local image (so iteration on test code doesn't
	// trigger a full cargo build per run).
	runtimeImageRepo  = "localhost/wasmcloud-wash"
	buildRuntimeImage = os.Getenv("BUILD_RUNTIME_IMAGE") == envBoolTrue
	skipRuntimeBuild  = os.Getenv("SKIP_RUNTIME_BUILD") == envBoolTrue
	// RUNTIME_LOG_LEVEL optionally sets the wash host's `--log-level`. When
	// unset (the default), the chart leaves the flag off and the host runs
	// at its built-in INFO level — matching production. Set to e.g. "debug"
	// when iterating on a failing run that needs the NatsMessaging plugin's
	// instrumentation in the diagnostic dump.
	runtimeLogLevel = os.Getenv("RUNTIME_LOG_LEVEL")

	// helmChartPath points to the runtime-operator Helm chart relative to the project dir (runtime-operator/)
	helmChartPath = "../charts/runtime-operator"

	runtimeImageTag = "canary-v2"
	// runtimeSupportsHostAliases indicates whether the runtime supports HostAliases,
	// which is required for testing with EndpointSlices.
	runtimeSupportsHostAliases = false
)

// TestE2E runs the end-to-end (e2e) test suite for the project. These tests execute in an isolated,
// temporary environment to validate project changes with the the purposed to be used in CI jobs.
// The default setup requires Kind, builds/loads the Manager Docker image locally, and deploys
// the full stack via Helm.
func TestE2E(t *testing.T) {
	RegisterFailHandler(Fail)
	_, _ = fmt.Fprintf(GinkgoWriter, "Starting operator integration test suite\n")
	RunSpecs(t, "e2e suite")
}

var _ = BeforeSuite(func() {
	if !skipImageBuild {
		By("building the operator image")
		cmd := exec.Command("make", "docker-build", fmt.Sprintf("IMG=%s", projectImage))
		_, err := utils.Run(cmd)
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to build the operator image")
	}

	By("loading the operator image into Kind")
	err := utils.LoadImageToKindClusterWithName(projectImage)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to load the operator image into Kind")

	if buildRuntimeImage {
		runtimeImageRef := fmt.Sprintf("%s:%s", runtimeImageRepo, operatorImageTag)
		if !skipRuntimeBuild {
			By("building the wash-runtime image from the local tree")
			// Repo root sits one level above runtime-operator/.
			cmd := exec.Command("docker", "build", "-t", runtimeImageRef, "..")
			_, err := utils.Run(cmd)
			ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to build the wash-runtime image")
		}

		By("loading the wash-runtime image into Kind")
		err := utils.LoadImageToKindClusterWithName(runtimeImageRef)
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to load the wash-runtime image into Kind")
	}

	By("installing the runtime-operator via Helm")
	// Build the full list of `--set key=value` values in one place; the
	// command line is assembled below.
	sets := []string{
		"operator.image.registry=",
		fmt.Sprintf("operator.image.repository=%s", operatorImageRepo),
		fmt.Sprintf("operator.image.tag=%s", operatorImageTag),
		"operator.image.pull_policy=Never",
		"gateway.image.tag=canary",
		"gateway.service.type=NodePort",
		"gateway.service.nodePort=30950",
		"runtime.hostGroups[0].name=default",
		"runtime.hostGroups[0].replicas=1",
		"runtime.hostGroups[0].service.type=ClusterIP",
		"runtime.hostGroups[0].http.enabled=true",
		"runtime.hostGroups[0].http.port=80",
		"runtime.hostGroups[0].webgpu.enabled=false",
		"runtime.hostGroups[0].resources.requests.memory=64Mi",
		"runtime.hostGroups[0].resources.requests.cpu=250m",
		"runtime.hostGroups[0].resources.limits.memory=512Mi",
		"runtime.hostGroups[0].resources.limits.cpu=500m",
		// Driven by RUNTIME_LOG_LEVEL env var; empty value leaves the
		// chart's `{{- if .logLevel }}` guard off, so wash uses INFO.
		fmt.Sprintf("runtime.hostGroups[0].logLevel=%s", runtimeLogLevel),
	}
	if buildRuntimeImage {
		// Point at the locally-built image and disable pull so kubelet
		// uses the kind-loaded copy.
		sets = append(sets,
			"runtime.image.registry=",
			fmt.Sprintf("runtime.image.repository=%s", runtimeImageRepo),
			fmt.Sprintf("runtime.image.tag=%s", operatorImageTag),
			"runtime.image.pull_policy=Never",
		)
	} else {
		sets = append(sets, fmt.Sprintf("runtime.image.tag=%s", runtimeImageTag))
	}

	helmArgs := make([]string, 0, 5+2*len(sets)+4)
	helmArgs = append(helmArgs, "upgrade", "--install", "--create-namespace", "-n", namespace)
	for _, s := range sets {
		helmArgs = append(helmArgs, "--set", s)
	}
	helmArgs = append(helmArgs, "--wait", "--timeout=5m", "operator-e2e", helmChartPath)

	cmd := exec.Command("helm", helmArgs...)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to install the runtime-operator via Helm")

	// Setup Prometheus and CertManager before the suite if not skipped and if not already installed
	if !skipPrometheusInstall {
		By("checking if prometheus is installed already")
		isPrometheusOperatorAlreadyInstalled = utils.IsPrometheusCRDsInstalled()
		if !isPrometheusOperatorAlreadyInstalled {
			_, _ = fmt.Fprintf(GinkgoWriter, "Installing Prometheus Operator...\n")
			Expect(utils.InstallPrometheusOperator()).To(Succeed(), "Failed to install Prometheus Operator")
		} else {
			_, _ = fmt.Fprintf(GinkgoWriter, "WARNING: Prometheus Operator is already installed. Skipping installation...\n")
		}
	}
	if !skipCertManagerInstall {
		By("checking if cert manager is installed already")
		isCertManagerAlreadyInstalled = utils.IsCertManagerCRDsInstalled()
		if !isCertManagerAlreadyInstalled {
			_, _ = fmt.Fprintf(GinkgoWriter, "Installing CertManager...\n")
			Expect(utils.InstallCertManager()).To(Succeed(), "Failed to install CertManager")
		} else {
			_, _ = fmt.Fprintf(GinkgoWriter, "WARNING: CertManager is already installed. Skipping installation...\n")
		}
	}
})

var _ = AfterSuite(func() {
	By("uninstalling the Helm release")
	cmd := exec.Command("helm", "delete", "-n", namespace, "operator-e2e")
	_, _ = utils.Run(cmd)

	// Teardown Prometheus and CertManager after the suite if not skipped and if they were not already installed
	if !skipPrometheusInstall && !isPrometheusOperatorAlreadyInstalled {
		_, _ = fmt.Fprintf(GinkgoWriter, "Uninstalling Prometheus Operator...\n")
		utils.UninstallPrometheusOperator()
	}
	if !skipCertManagerInstall && !isCertManagerAlreadyInstalled {
		_, _ = fmt.Fprintf(GinkgoWriter, "Uninstalling CertManager...\n")
		utils.UninstallCertManager()
	}
})
