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
	// trigger a full cargo build per run). In CI the wash.yml legs supply the
	// host image(s) as pre-built tarballs (SKIP_RUNTIME_BUILD=true) and select
	// them per hostgroup via E2E_RUNTIME_IMAGE_TAG / E2E_REGISTRY_HOST_IMAGE_TAG.
	runtimeImageRepo  = "localhost/wasmcloud-wash"
	buildRuntimeImage = os.Getenv("BUILD_RUNTIME_IMAGE") == envBoolTrue
	skipRuntimeBuild  = os.Getenv("SKIP_RUNTIME_BUILD") == envBoolTrue
	// defaultHostImageTag is the wash-runtime image tag the default (fixture)
	// hostgroup runs. The wash.yml legs set E2E_RUNTIME_IMAGE_TAG to the release
	// or all-features build; unset (local) uses the locally-built operatorImageTag.
	defaultHostImageTag = envOrDefault("E2E_RUNTIME_IMAGE_TAG", operatorImageTag)
	// registryHostImageTag is the tag the `registry` hostgroup runs. The
	// oci-registry needs the async wasmcloud:blobstore plugin, so it must be an
	// all-features build; the wash.yml release leg sets E2E_REGISTRY_HOST_IMAGE_TAG
	// to it. Defaults to the default host image — local runs and the all-features
	// leg use a single (feature) image for both hostgroups.
	registryHostImageTag = envOrDefault("E2E_REGISTRY_HOST_IMAGE_TAG", defaultHostImageTag)
	// defaultHostAllFeatures reports whether the fixture host can run feature-only
	// components (the implements spec) — true when it shares the all-features
	// image the registry host uses (local + the all-features leg; not the release
	// leg, whose default host is the shipped build).
	defaultHostAllFeatures = defaultHostImageTag == registryHostImageTag
	// RUNTIME_LOG_LEVEL optionally sets the wash host's `--log-level`. When
	// unset (the default), the chart leaves the flag off and the host runs
	// at its built-in INFO level — matching production. Set to e.g. "debug"
	// when iterating on a failing run that needs the NatsMessaging plugin's
	// instrumentation in the diagnostic dump.
	runtimeLogLevel = os.Getenv("RUNTIME_LOG_LEVEL")

	// inClusterRegistry serves the fixture images from an in-cluster
	// oci-registry (the examples/oci-registry wasm component) instead of
	// published ghcr refs. The registry itself needs an all-features host (the
	// async wasmcloud:blobstore plugin), so it runs on a dedicated `registry`
	// hostgroup pinned to that image — but the p2 fixtures it serves are pullable
	// by the shipped (release) host too, so BOTH wash.yml legs enable this. Off
	// for the canary runtime-operator.yml job and plain local runs, where the
	// registry-served specs self-skip and the http specs use published refs.
	//
	// TODO(wash release): once the async wasmcloud:blobstore plugin ships in the
	// default wash build (not just under the wasm_component_model_implements
	// feature), the registry can run on the shipped host — dropping this
	// all-features-only gating so every leg serves fixtures from the registry.
	inClusterRegistry = os.Getenv("E2E_IN_CLUSTER_REGISTRY") == envBoolTrue

	// helmChartPath points to the runtime-operator Helm chart relative to the project dir (runtime-operator/)
	helmChartPath = "../charts/runtime-operator"

	// canary is published on every merge to main
	runtimeImageTag = "canary"
	// runtimeSupportsHostAliases gates the HTTP traffic assertion in the
	// EndpointSlice context. That test relies on the wash runtime
	// resolving the Service hostname via /etc/hosts. EndpointSlice
	// creation itself is exercised regardless of this flag.
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
	// The in-cluster registry runs on an all-features host image; without
	// BUILD_RUNTIME_IMAGE the run falls back to the default-features canary,
	// which can't run the registry (the async wasmcloud:blobstore plugin is
	// feature-gated). Fail fast with a clear message rather than a 5m wait.
	if inClusterRegistry && !buildRuntimeImage {
		Fail("E2E_IN_CLUSTER_REGISTRY=true requires BUILD_RUNTIME_IMAGE=true " +
			"(the oci-registry needs an all-features host image)")
	}

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
		if !skipRuntimeBuild {
			By("building the wash-runtime image from the local tree")
			// Repo root sits one level above runtime-operator/. Build wash with
			// `(implements ..)` multiplexing enabled. A local run builds a single
			// feature image tagged defaultHostImageTag, which both the default and
			// registry hostgroups use.
			ref := fmt.Sprintf("%s:%s", runtimeImageRepo, defaultHostImageTag)
			cmd := exec.Command("docker", "build",
				"--build-arg", "CARGO_FEATURES=wasm_component_model_implements",
				"-t", ref, "..")
			_, err := utils.Run(cmd)
			ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to build the wash-runtime image")
		}

		// kind-load each distinct host image the release references: the default
		// (fixture) host, and — when it differs (the release leg) — the
		// all-features registry host. In CI these were docker-loaded from the
		// wash-image tarballs; locally the default one was just built.
		for _, tag := range dedupeStrings(defaultHostImageTag, registryHostImageTag) {
			By(fmt.Sprintf("loading the wash-runtime image %s into Kind", tag))
			err := utils.LoadImageToKindClusterWithName(fmt.Sprintf("%s:%s", runtimeImageRepo, tag))
			ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to load the wash-runtime image into Kind")
		}
	}

	By("installing the runtime-operator via Helm")
	sets := buildBaseHelmSets()

	helmArgs := make([]string, 0, 5+2*len(sets)+4)
	helmArgs = append(helmArgs, "upgrade", "--install", "--create-namespace", "-n", namespace)
	for _, s := range sets {
		helmArgs = append(helmArgs, "--set", s)
	}
	helmArgs = append(helmArgs, "--wait", "--timeout=5m", "operator-e2e", helmChartPath)

	cmd := exec.Command("helm", helmArgs...)
	_, err = utils.Run(cmd)
	ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to install the runtime-operator via Helm")

	// On the all-features leg, build the fixture components, deploy the
	// in-cluster oci-registry onto the `registry` hostgroup, and push the
	// fixtures into it. The specs then pull those images by their in-cluster ref
	// (registryRef), so no image is published out of band. Kept as a Make target
	// so the same flow runs locally and in CI; it must run after the Helm install
	// brought the hostgroups up. Skipped when inClusterRegistry is off (the
	// registry needs an all-features host that other legs don't run).
	if inClusterRegistry {
		By("building and pushing e2e fixture images into the in-cluster registry")
		cmd = exec.Command("make", "e2e-images")
		_, err = utils.Run(cmd)
		ExpectWithOffset(1, err).NotTo(HaveOccurred(), "Failed to build/push e2e fixture images")
	}

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

// registryImageTag is the tag the `e2e-images` xtask pushes every fixture
// under. Kept in sync with TAG in xtask/src/e2e_images.rs.
const registryImageTag = "e2e"

// registryRef returns the in-cluster pull ref for a fixture that
// `make e2e-images` built and pushed. The insecure hostgroups resolve it over
// plain HTTP via the oci-registry Service DNS, so specs never depend on an
// image published out of band.
func registryRef(name string) string {
	return fmt.Sprintf("oci-registry.%s.svc/fixtures/%s:%s", namespace, name, registryImageTag)
}

// envOrDefault returns the env var value, or def when unset/empty.
func envOrDefault(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

// dedupeStrings returns vals with duplicates removed, preserving order.
func dedupeStrings(vals ...string) []string {
	seen := make(map[string]bool, len(vals))
	out := make([]string, 0, len(vals))
	for _, v := range vals {
		if !seen[v] {
			seen[v] = true
			out = append(out, v)
		}
	}
	return out
}

// extraHostGroupIndex is the hostGroups index the helm-upgrade scenarios
// (tenant, scoped) append their group at. When inClusterRegistry is on, the
// registry occupies [1], so they use [2]; otherwise [1].
func extraHostGroupIndex() int {
	if inClusterRegistry {
		return 2
	}
	return 1
}

// buildBaseHelmSets returns the `--set` values used to install the
// runtime-operator chart for the e2e suite. It always configures `hostGroups[0]`
// (the `default` group the fixture specs exercise); when inClusterRegistry is
// on it also makes that group insecure and adds a secure `hostGroups[1]`
// (`registry`) that runs the in-cluster oci-registry. It is shared by the
// initial install in BeforeSuite and the helm upgrade scenarios that append a
// further hostGroup (at extraHostGroupIndex()).
func buildBaseHelmSets() []string {
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
	if inClusterRegistry {
		// The default hostgroup pulls the test fixtures from the in-cluster
		// oci-registry over plain HTTP. The host's insecure flag is global (it
		// forces HTTP for every registry), which is why the registry component
		// itself lives on a separate, secure hostgroup below. Added on both
		// wash.yml legs (each runs a registry); off for the canary
		// runtime-operator.yml job and plain local runs, which keep the default
		// hostgroup on HTTPS and pull published ghcr images.
		//
		// TODO(wash release): once wash supports per-registry insecure config
		// (allow HTTP for the in-cluster registry only, not globally), the
		// registry can share the default hostgroup — dropping this second
		// hostgroup and the extraHostGroupIndex() shuffle in the tenant/scoped
		// specs.
		sets = append(sets,
			"runtime.hostGroups[0].extraArgs[0]=--allow-insecure-registries",
			// hostGroups[1]: the `registry` hostgroup. Stays on HTTPS so it can
			// pull the oci-registry component from ghcr. It runs only the
			// oci-registry workload (testdata/oci-registry.yaml, hostSelector:
			// hostgroup=registry). The oci-registry exports a p3 async
			// wasi:http/handler and imports the async wasmcloud:blobstore plugin;
			// the all-features engine enables the component-model-async proposal
			// by default (crates/wash-runtime/src/engine/mod.rs build()), so no
			// extra host flag is required.
			"runtime.hostGroups[1].name=registry",
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
		// The registry host must be an all-features build (for the async
		// blobstore plugin). When the default host isn't one — the release leg,
		// where it's the shipped image — override just this hostgroup's image tag
		// so the fixture host can still be validated pulling from the registry.
		if buildRuntimeImage && registryHostImageTag != defaultHostImageTag {
			sets = append(sets, fmt.Sprintf("runtime.hostGroups[1].image.tag=%s", registryHostImageTag))
		}
	}
	if buildRuntimeImage {
		// Point at the locally-built image and disable pull so kubelet
		// uses the kind-loaded copy.
		sets = append(sets,
			"runtime.image.registry=",
			fmt.Sprintf("runtime.image.repository=%s", runtimeImageRepo),
			fmt.Sprintf("runtime.image.tag=%s", defaultHostImageTag),
			"runtime.image.pull_policy=Never",
		)
	} else {
		// Use IfNotPresent so kubelet prefers a locally-loaded image
		// (e.g. one built and `kind load`ed by the developer) over pulling
		// the canary tag, which is published on every merge to main but
		// may not be available — or may lag the local tree — in offline
		// or pre-merge runs.
		sets = append(sets,
			fmt.Sprintf("runtime.image.tag=%s", runtimeImageTag),
			"runtime.image.pull_policy=IfNotPresent",
		)
	}
	return sets
}

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
