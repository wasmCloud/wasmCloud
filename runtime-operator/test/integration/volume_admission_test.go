package integration

import (
	"context"
	"fmt"

	. "github.com/onsi/ginkgo/v2"
	. "github.com/onsi/gomega"
	apierrors "k8s.io/apimachinery/pkg/api/errors"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const volumeName = "data"

// The API server refuses volumes that cannot mean one thing: two types, of
// which the operator could send only one, or names that are not unique DNS
// labels as documented.
var _ = Describe("Workload volume admission", func() {
	hostPath := &runtimev1alpha1.HostPathVolume{Path: "/srv/data"}
	ephemeral := &runtimev1alpha1.EphemeralVolume{}
	seq := 0

	create := func(volumes ...runtimev1alpha1.Volume) error {
		seq++
		w := newWorkload(fmt.Sprintf("volume-admission-%d", seq), "volume-admission-host")
		w.Spec.Volumes = volumes
		return k8sClient.Create(context.Background(), w)
	}

	DescribeTable("refuses",
		func(volumes []runtimev1alpha1.Volume, message string) {
			err := create(volumes...)
			Expect(apierrors.IsInvalid(err)).To(BeTrue(), "admitted, or failed for another reason: %v", err)
			Expect(err.Error()).To(ContainSubstring(message))
		},
		Entry("a volume with both types",
			[]runtimev1alpha1.Volume{{Name: volumeName, EphemeralVolume: ephemeral, HostPathVolume: hostPath}},
			"ephemeral and hostPath are mutually exclusive"),
		Entry("two volumes with one name",
			[]runtimev1alpha1.Volume{
				{Name: volumeName, HostPathVolume: hostPath},
				{Name: volumeName, EphemeralVolume: ephemeral},
			},
			"Duplicate value"),
		Entry("a name that is not a DNS label",
			[]runtimev1alpha1.Volume{{Name: "Not_A_Label", EphemeralVolume: ephemeral}},
			"spec.volumes[0].name"),
	)

	DescribeTable("admits",
		func(volumes []runtimev1alpha1.Volume) {
			Expect(create(volumes...)).To(Succeed())
		},
		Entry("an ephemeral volume", []runtimev1alpha1.Volume{{Name: "scratch", EphemeralVolume: ephemeral}}),
		// No type is the documented ephemeral default, so templates written
		// that way keep rolling out.
		Entry("a volume with no type", []runtimev1alpha1.Volume{{Name: volumeName}}),
		Entry("several host paths", []runtimev1alpha1.Volume{
			{Name: "a", HostPathVolume: hostPath}, {Name: "b", HostPathVolume: hostPath},
		}),
		Entry("no volumes", nil),
	)
})
