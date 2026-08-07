package e2e

import (
	"fmt"
	"os/exec"
	"time"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"

	"go.wasmcloud.dev/runtime-operator/v2/test/utils"
)

// The in-cluster registry authenticates every request, and the credentials it
// checks against reach it through `wasmcloud:secrets` — declared as interface
// config on its WorkloadDeployment, captured by the host's native secrets
// plugin at bind time, and read by the component with `store.get` + `reveal`.
//
// The rest of the suite exercises that path implicitly: fixtures are pushed and
// pulled with credentials, so nothing works if the secrets never arrive. What
// that cannot show is the other half — that a caller *without* credentials is
// actually turned away. A registry serving anonymously would pass every other
// spec just as well, which is exactly the failure worth guarding against: the
// component denies all requests when its secrets backend comes up empty, so
// "auth is enforced" and "secrets were delivered" are the same assertion made
// from opposite sides.
//
// Runs wherever the registry does (the in-cluster registry flow); self-skips
// otherwise.
var _ = Describe("Registry Auth", func() {
	// The gateway routes by Host header, and the registry registers this as its
	// ingress hostname (see testdata/oci-registry.yaml).
	const registryHost = "oci-registry"

	BeforeEach(func() {
		if !inClusterRegistry {
			Skip("skipping registry auth e2e (needs the in-cluster registry)")
		}
	})

	// `/v2/` is where an OCI client starts, and the registry authenticates it
	// like everything else — so it is both the cheapest probe and the one that
	// decides whether a client believes it must authenticate at all.
	probe := func(args ...string) (string, error) {
		full := append([]string{
			"-s", "-o", "/dev/null", "-w", "%{http_code}",
			"-H", "Host: " + registryHost,
		}, args...)
		full = append(full, "http://localhost:80/v2/")
		return utils.Run(exec.Command("curl", full...))
	}

	It("serves the API only to an authenticated caller", func() {
		By("rejecting a caller with no credentials")
		// Eventually, not Expect: this is the first thing to touch the registry
		// in this spec, and the route may still be propagating.
		Eventually(func(g Gomega) {
			code, err := probe()
			g.Expect(err).NotTo(HaveOccurred())
			g.Expect(code).To(Equal("401"),
				"an unauthenticated caller must be refused — a registry that "+
					"answers here is serving anonymously, which means its "+
					"wasmcloud:secrets credentials never arrived")
		}).WithTimeout(2 * time.Minute).Should(Succeed())

		By("advertising Basic auth so a client knows how to authenticate")
		out, err := utils.Run(exec.Command("curl", "-s", "-i",
			"-H", "Host: "+registryHost, "http://localhost:80/v2/"))
		Expect(err).NotTo(HaveOccurred())
		// Matched case-insensitively because header names are: the component
		// sends `WWW-Authenticate` and it arrives here as `Www-Authenticate`.
		// The scheme matters as much as the header — a challenge naming
		// anything other than Basic sends a client down a path this registry
		// does not implement.
		Expect(out).To(MatchRegexp(`(?i)www-authenticate:\s*Basic`),
			"a 401 without a Basic challenge leaves an OCI client no way to proceed")

		By("rejecting a caller with the wrong password")
		code, err := probe("-u", registryUser+":not-the-password")
		Expect(err).NotTo(HaveOccurred())
		Expect(code).To(Equal("401"))

		By("accepting the credentials the registry was given")
		// The same pair the push side uses, delivered to the component through
		// the secrets plugin — so this passing is what proves the delivery.
		code, err = probe("-u", fmt.Sprintf("%s:%s", registryUser, registryPassword))
		Expect(err).NotTo(HaveOccurred())
		Expect(code).To(Equal("200"))
	})
})
