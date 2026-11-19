package runtime

import (
	"context"
	"testing"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// A host's hostname resolves only to a host Pod: another Pod with the same
// name or hostname (a sidecar, a job) must not receive its traffic.
func TestFindPodByHostnameMatchesOnlyHostPods(t *testing.T) {
	const hostname = "wasmcloud-host-0"
	hostLabels := map[string]string{HostPodLabel: testHostGroup}
	impostor := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{Name: hostname, Namespace: testNamespace},
		Spec:       corev1.PodSpec{Hostname: hostname},
	}
	otherHost := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{Name: "wasmcloud-host-1", Namespace: testNamespace, Labels: hostLabels},
	}
	// A pinned hostname differs from the Pod's name.
	pinned := &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{Name: "hostgroup-abc12", Namespace: testNamespace, Labels: hostLabels},
		Spec:       corev1.PodSpec{Hostname: hostname},
	}
	r := newRouteReconciler(t, impostor, otherHost, pinned)

	pod, err := r.findPodByHostname(context.Background(), hostname)
	if err != nil {
		t.Fatalf("findPodByHostname: %v", err)
	}
	if pod.Name != pinned.Name {
		t.Fatalf("resolved %q to pod %q, want %q", hostname, pod.Name, pinned.Name)
	}

	if pod, err := r.findPodByHostname(context.Background(), "absent"); err == nil {
		t.Fatalf("resolved an unknown hostname to pod %q", pod.Name)
	}
}
