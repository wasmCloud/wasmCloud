package runtime

import (
	"context"
	"slices"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

// A Pod's IP may have been reused from the moment it was told to go, which is
// its deletion deadline less the grace period it was given.
func TestPodCondemnedAt(t *testing.T) {
	deadline := metav1.NewTime(time.Unix(1_700_000_030, 0))
	grace := int64(30)
	for _, tc := range []struct {
		name string
		pod  corev1.Pod
		want time.Time
	}{
		{name: "not terminating"},
		{
			name: "with grace",
			pod:  corev1.Pod{ObjectMeta: metav1.ObjectMeta{DeletionTimestamp: &deadline, DeletionGracePeriodSeconds: &grace}},
			want: time.Unix(1_700_000_000, 0),
		},
		{
			// Force deletion: the deadline is the request itself.
			name: "without grace",
			pod:  corev1.Pod{ObjectMeta: metav1.ObjectMeta{DeletionTimestamp: &deadline}},
			want: deadline.Time,
		},
	} {
		if got := podCondemnedAt(&tc.pod); !got.Equal(tc.want) {
			t.Errorf("%s: podCondemnedAt = %v, want %v", tc.name, got, tc.want)
		}
	}
}

// Only host Pods with an IP are indexed by it: other Pods in the namespace can
// hold an IP a host Pod had, and must not keep that host looking alive.
func TestHostPodIPIndexValue(t *testing.T) {
	hostLabels := map[string]string{HostPodLabel: testHostGroup}
	for _, tc := range []struct {
		name string
		obj  client.Object
		want []string
	}{
		{"host pod", &corev1.Pod{ObjectMeta: metav1.ObjectMeta{Labels: hostLabels}, Status: corev1.PodStatus{PodIP: testPodIP}}, []string{testPodIP}},
		{"host pod without an IP", &corev1.Pod{ObjectMeta: metav1.ObjectMeta{Labels: hostLabels}}, nil},
		{"other pod", &corev1.Pod{Status: corev1.PodStatus{PodIP: testPodIP}}, nil},
		{"not a pod", &runtimev1alpha1.Host{Hostname: testPodIP}, nil},
	} {
		if got := hostPodIPIndexValue(tc.obj); !slices.Equal(got, tc.want) {
			t.Errorf("%s: index = %v, want %v", tc.name, got, tc.want)
		}
	}
}

// A heartbeat with no IP cannot belong to a draining Pod, and is answered
// without a lookup.
func TestHostPodDrainingWithoutIP(t *testing.T) {
	lists := 0
	c := hostPodClientBuilder(t).WithInterceptorFuncs(interceptor.Funcs{
		List: func(context.Context, client.WithWatch, client.ObjectList, ...client.ListOption) error {
			lists++
			return nil
		},
	}).Build()
	draining, err := hostPodDraining(context.Background(), c, "")
	if draining || err != nil || lists != 0 {
		t.Fatalf("hostPodDraining(\"\") = %v, %v after %d lists", draining, err, lists)
	}
}
