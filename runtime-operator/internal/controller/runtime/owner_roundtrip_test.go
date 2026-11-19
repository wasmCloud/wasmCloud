package runtime

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/google/go-cmp/cmp"
	corev1 "k8s.io/api/core/v1"
	apiequality "k8s.io/apimachinery/pkg/api/equality"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/envtest"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// Owner checks compare stored objects with the template the operator holds
// in memory, so the API server must store a template as it was sent: a
// default or a mutation would refuse every replica.
func TestAStoredTemplateIsTheOneSent(t *testing.T) {
	if os.Getenv("KUBEBUILDER_ASSETS") == "" {
		t.Skip("KUBEBUILDER_ASSETS not set; run via make test for envtest")
	}
	env := &envtest.Environment{
		CRDDirectoryPaths:     []string{filepath.Join("..", "..", "..", "config", "crd", "bases")},
		ErrorIfCRDPathMissing: true,
	}
	cfg, err := env.Start()
	if err != nil {
		t.Fatalf("start envtest: %v", err)
	}
	t.Cleanup(func() {
		if err := env.Stop(); err != nil {
			t.Errorf("stop envtest: %v", err)
		}
	})
	scheme := runtime.NewScheme()
	if err := corev1.AddToScheme(scheme); err != nil {
		t.Fatal(err)
	}
	if err := runtimev1alpha1.AddToScheme(scheme); err != nil {
		t.Fatal(err)
	}
	c, err := client.New(cfg, client.Options{Scheme: scheme})
	if err != nil {
		t.Fatal(err)
	}
	ctx := t.Context()
	if err := c.Create(ctx, &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: testTenant}}); err != nil {
		t.Fatal(err)
	}

	_, fixture, _ := placementFixture(t)
	template := runtimev1alpha1.WorkloadReplicaTemplate{
		Labels:      map[string]string{"tier": "billing"},
		Annotations: map[string]string{"billing.example/owner": "finance"},
		Spec:        *fixture.Spec.DeepCopy(),
	}
	template.Spec.HostID = ""
	sent := template.DeepCopy()
	set := &runtimev1alpha1.WorkloadReplicaSet{
		ObjectMeta: metav1.ObjectMeta{Name: "billing-set", Namespace: testTenant},
		Spec:       runtimev1alpha1.WorkloadReplicaSetSpec{Template: template},
	}
	if err := c.Create(ctx, set); err != nil {
		t.Fatal(err)
	}
	var stored runtimev1alpha1.WorkloadReplicaSet
	if err := c.Get(ctx, client.ObjectKeyFromObject(set), &stored); err != nil {
		t.Fatal(err)
	}
	if got, want := stored.Spec.Template.Digest(), sent.Digest(); got != want {
		t.Fatalf("stored template digest %s, sent %s:\n%s", got, want, cmp.Diff(*sent, stored.Spec.Template))
	}

	workload := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{Name: "billing-replica", Namespace: testTenant},
		Spec:       *stored.Spec.Template.Spec.DeepCopy(),
	}
	if err := c.Create(ctx, workload); err != nil {
		t.Fatal(err)
	}
	var replica runtimev1alpha1.Workload
	if err := c.Get(ctx, client.ObjectKeyFromObject(workload), &replica); err != nil {
		t.Fatal(err)
	}
	if !apiequality.Semantic.DeepEqual(replica.Spec, stored.Spec.Template.Spec) {
		t.Fatalf("a stored replica's spec is not its set's template:\n%s", cmp.Diff(stored.Spec.Template.Spec, replica.Spec))
	}
}
