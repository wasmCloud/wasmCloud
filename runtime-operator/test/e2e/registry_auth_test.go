package e2e

import (
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

// The in-cluster registry authenticates every request, and the credentials it
// checks against reach it through `wasmcloud:secrets`: declared as interface
// config on its WorkloadDeployment, captured by the host's native secrets
// plugin at bind time, and read by the component with `store.get` + `reveal`.
//
// The rest of the suite exercises that path implicitly: fixtures are pushed and
// pulled with credentials, so nothing works if the secrets never arrive. What
// that cannot show is the other half: that a caller *without* credentials is
// actually turned away. A registry serving anonymously would pass every other
// spec just as well, which is exactly the failure worth guarding against: the
// component denies all requests when its secrets backend comes up empty, so
// "auth is enforced" and "secrets were delivered" are the same assertion made
// from opposite sides.
//
// Runs wherever the registry does (the in-cluster registry flow); self-skips
// otherwise.
var _ = Describe("Registry Auth", func() {
	var (
		baseURL string
		caPath  string
		forward *exec.Cmd
	)

	BeforeEach(func() {
		if !inClusterRegistry {
			Skip("skipping registry auth e2e (needs the in-cluster registry)")
		}

		// Straight to the registry's host rather than through the gateway: the
		// registry hostgroup serves TLS, and the gateway proxies to its
		// upstreams in plaintext. Reaching it the way `make e2e-images` does
		// also keeps this spec measuring the registry rather than the routing
		// in front of it.
		var err error
		caPath, err = fetchRegistryCA()
		Expect(err).NotTo(HaveOccurred())

		port, err := freeLocalPort()
		Expect(err).NotTo(HaveOccurred())
		baseURL = fmt.Sprintf("https://127.0.0.1:%d/v2/", port)

		forward = exec.Command("kubectl", "port-forward", "--address", "127.0.0.1",
			"-n", namespace, "deployment/hostgroup-registry", fmt.Sprintf("%d:80", port))
		Expect(forward.Start()).To(Succeed())

		// The forward needs a moment before it accepts; any status at all means
		// it is up, so this waits on connectivity rather than on a code.
		Eventually(func() error {
			_, err := utils.Run(exec.Command("curl", "-s", "-o", "/dev/null",
				"--cacert", caPath, baseURL))
			return err
		}).WithTimeout(time.Minute).WithPolling(2 * time.Second).Should(Succeed())
	})

	AfterEach(func() {
		if forward != nil && forward.Process != nil {
			_ = forward.Process.Kill()
			_ = forward.Wait()
		}
	})

	// `/v2/` is where an OCI client starts, and the registry authenticates it
	// like everything else, so it is both the cheapest probe and the one that
	// decides whether a client believes it must authenticate at all.
	probe := func(args ...string) (string, error) {
		full := append([]string{
			"-s", "-o", "/dev/null", "-w", "%{http_code}",
			"--cacert", caPath,
		}, args...)
		full = append(full, baseURL)
		return utils.Run(exec.Command("curl", full...))
	}

	It("serves the API only to an authenticated caller", func() {
		By("rejecting a caller with no credentials")
		code, err := probe()
		Expect(err).NotTo(HaveOccurred())
		Expect(code).To(Equal("401"),
			"an unauthenticated caller must be refused: a registry that "+
				"answers here is serving anonymously, which means its "+
				"wasmcloud:secrets credentials never arrived")

		By("advertising Basic auth so a client knows how to authenticate")
		out, err := utils.Run(exec.Command("curl", "-s", "-i", "--cacert", caPath, baseURL))
		Expect(err).NotTo(HaveOccurred())
		// Matched case-insensitively because header names are: the component
		// sends `WWW-Authenticate` and it arrives here as `Www-Authenticate`.
		// The scheme matters as much as the header: a challenge naming
		// anything other than Basic sends a client down a path this registry
		// does not implement.
		Expect(out).To(MatchRegexp(`(?i)www-authenticate:\s*Basic`),
			"a 401 without a Basic challenge leaves an OCI client no way to proceed")

		By("rejecting a caller with the wrong password")
		code, err = probe("-u", registryUser+":not-the-password")
		Expect(err).NotTo(HaveOccurred())
		Expect(code).To(Equal("401"))

		By("accepting the credentials the registry was given")
		// The same pair the push side uses, delivered to the component through
		// the secrets plugin, so this passing is what proves the delivery.
		code, err = probe("-u", fmt.Sprintf("%s:%s", registryUser, registryPassword))
		Expect(err).NotTo(HaveOccurred())
		Expect(code).To(Equal("200"))
	})
})

// fetchRegistryCA writes the chart's CA certificate, the one that signed the
// registry's serving certificate, somewhere curl can be pointed at it. A
// go-template decodes the Secret's base64 in kubectl, which a jsonpath cannot.
func fetchRegistryCA() (string, error) {
	pem, err := utils.Run(exec.Command("kubectl", "get", "secret", "wasmcloud-ca",
		"-n", namespace, "-o", `go-template={{index .data "tls.crt" | base64decode}}`))
	if err != nil {
		return "", err
	}
	path := filepath.Join(os.TempDir(), "wasmcloud-e2e-registry-auth-ca.pem")
	if err := os.WriteFile(path, []byte(pem), 0o600); err != nil {
		return "", err
	}
	return path, nil
}

// freeLocalPort asks the OS for an unused port. The obvious constants are a
// poor bet on a developer machine: on macOS, AirPlay Receiver holds 5000 and
// 7000 and answers on them, so a registry that never came up would present as
// one serving errors.
func freeLocalPort() (int, error) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, err
	}
	defer func() { _ = listener.Close() }()
	return listener.Addr().(*net.TCPAddr).Port, nil
}
