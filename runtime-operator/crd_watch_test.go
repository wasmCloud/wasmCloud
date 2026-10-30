package runtime_operator

import (
	"context"
	"sync"
	"testing"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
)

var testGV = schema.GroupVersion{Group: "runtime.wasmcloud.dev", Version: "v1alpha1"}

// fakeResources serves one GroupVersion's kinds; nil kinds means the group is missing.
type fakeResources struct {
	mu    sync.Mutex
	kinds []string
}

func (f *fakeResources) set(kinds ...string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.kinds = kinds // set() with no kinds leaves nil: the group is missing
}

func (f *fakeResources) ServerResourcesForGroupVersion(gv string) (*metav1.APIResourceList, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.kinds == nil {
		return nil, apierrors.NewNotFound(schema.GroupResource{}, gv)
	}
	list := &metav1.APIResourceList{GroupVersion: gv}
	for _, kind := range f.kinds {
		list.APIResources = append(list.APIResources, metav1.APIResource{Kind: kind})
	}
	return list, nil
}

func newCRDWatch(kinds ...string) (*crdWatch, *fakeResources) {
	fake := &fakeResources{kinds: kinds}
	return &crdWatch{discovery: fake, gv: testGV, kind: "WorkloadDaemonSet", interval: 10 * time.Millisecond}, fake
}

func TestCRDWatch_Installed(t *testing.T) {
	w, fake := newCRDWatch("Workload")
	if ok, err := w.installed(); err != nil || ok {
		t.Fatalf("expected not installed, got ok=%v err=%v", ok, err)
	}

	// A group with no resources at all is a NotFound, not an error.
	fake.set()
	if ok, err := w.installed(); err != nil || ok {
		t.Fatalf("expected not installed for a missing group, got ok=%v err=%v", ok, err)
	}

	fake.set("Workload", "WorkloadDaemonSet")
	if ok, err := w.installed(); err != nil || !ok {
		t.Fatalf("expected installed, got ok=%v err=%v", ok, err)
	}
}

// Start fails once the CRD appears, so the manager exits and the pod
// restarts with the controller registered.
func TestCRDWatch_StartFailsOnceInstalled(t *testing.T) {
	w, fake := newCRDWatch("Workload")
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	done := make(chan error, 1)
	go func() { done <- w.Start(ctx) }()

	select {
	case err := <-done:
		t.Fatalf("Start returned before the CRD was installed: %v", err)
	case <-time.After(50 * time.Millisecond):
	}

	fake.set("Workload", "WorkloadDaemonSet")
	if err := <-done; err == nil {
		t.Fatal("expected Start to return an error once the CRD is installed")
	}
}

func TestCRDWatch_StartStopsWithManager(t *testing.T) {
	w, _ := newCRDWatch("Workload")
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := w.Start(ctx); err != nil {
		t.Fatalf("expected nil on shutdown, got %v", err)
	}
}
