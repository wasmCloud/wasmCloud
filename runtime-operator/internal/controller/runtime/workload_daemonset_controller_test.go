package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	"go.wasmcloud.dev/runtime-operator/v2/api/condition"
	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const (
	testDaemonSetName      = "echo-everywhere"
	testDaemonSetNamespace = "default"
	dsHostA                = "ds-host-a"
	dsHostB                = "ds-host-b"
	dsHostC                = "ds-host-c"
	dsOtherGroup           = "other"
)

func daemonSetHostObj(hostID string, ready bool, hostLabels map[string]string) *runtimev1alpha1.Host {
	host := &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{
			Name:      hostID,
			Namespace: testNamespace,
			Labels:    hostLabels,
		},
		HostID: hostID,
	}
	if ready {
		host.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
	}
	return host
}

func defaultGroupHost(hostID string) *runtimev1alpha1.Host {
	return daemonSetHostObj(hostID, true, map[string]string{testHostGroupLabel: testHostGroup})
}

func testDaemonSet(replicas int32) *runtimev1alpha1.WorkloadDaemonSet {
	ds := &runtimev1alpha1.WorkloadDaemonSet{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testDaemonSetNamespace,
			Name:      testDaemonSetName,
			UID:       "ds-uid",
		},
		Spec: runtimev1alpha1.WorkloadDaemonSetSpec{
			Replicas: &replicas,
			Template: runtimev1alpha1.WorkloadReplicaTemplate{
				Spec: runtimev1alpha1.WorkloadSpec{
					HostSelector: map[string]string{testHostGroupLabel: testHostGroup},
					Components: []runtimev1alpha1.WorkloadComponent{
						{Name: "echo", Image: "echo:v1"},
					},
				},
			},
		},
	}
	ds.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDaemonSetConditionArtifact))
	return ds
}

func newDaemonSetReconciler(t *testing.T, objs ...client.Object) *WorkloadDaemonSetReconciler {
	t.Helper()
	s := newScaleTestScheme(t)

	daemonSetGvk, err := gvkForType(&runtimev1alpha1.WorkloadDaemonSet{}, s)
	if err != nil {
		t.Fatalf("gvk: %v", err)
	}

	c := fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.Workload{}, workloadDaemonSetNameIndex,
			func(obj client.Object) []string {
				if name, ok := isOwnedByController(obj, daemonSetGvk); ok {
					return []string{name}
				}
				return nil
			}).
		Build()

	return &WorkloadDaemonSetReconciler{
		Client:            c,
		Scheme:            s,
		OperatorNamespace: testNamespace,
		AllowSharedHosts:  true,
	}
}

// workloadsByHost lists the DaemonSet's live Workloads keyed by pinned HostID.
func workloadsByHost(t *testing.T, r *WorkloadDaemonSetReconciler) map[string][]runtimev1alpha1.Workload {
	t.Helper()
	workloads := &runtimev1alpha1.WorkloadList{}
	if err := r.List(context.Background(), workloads,
		client.InNamespace(testDaemonSetNamespace),
		client.MatchingFields{workloadDaemonSetNameIndex: testDaemonSetName},
	); err != nil {
		t.Fatalf("list workloads: %v", err)
	}
	byHost := map[string][]runtimev1alpha1.Workload{}
	for _, w := range workloads.Items {
		byHost[w.Spec.HostID] = append(byHost[w.Spec.HostID], w)
	}
	return byHost
}

func schedule(t *testing.T, r *WorkloadDaemonSetReconciler, ds *runtimev1alpha1.WorkloadDaemonSet) {
	t.Helper()
	if err := r.reconcileSchedule(context.Background(), ds); err != nil {
		t.Fatalf("reconcileSchedule: %v", err)
	}
}

// markAllAvailable sets Ready=True on every Workload the DaemonSet owns.
func markAllAvailable(t *testing.T, r *WorkloadDaemonSetReconciler) {
	t.Helper()
	for _, workloads := range workloadsByHost(t, r) {
		for i := range workloads {
			w := &workloads[i]
			w.Status.SetConditions(condition.ReadyCondition(condition.TypeReady))
			if err := r.Update(context.Background(), w); err != nil {
				t.Fatalf("update workload status: %v", err)
			}
		}
	}
}

func assertPerHost(t *testing.T, byHost map[string][]runtimev1alpha1.Workload, want map[string]int) {
	t.Helper()
	if len(byHost) != len(want) {
		t.Fatalf("expected workloads on hosts %v, got %d hosts: %v", want, len(byHost), byHost)
	}
	for hostID, n := range want {
		if got := len(byHost[hostID]); got != n {
			t.Fatalf("host %s: expected %d workloads, got %d", hostID, n, got)
		}
	}
}

func TestDaemonSetSchedule_RunsReplicasOnEveryMatchingHost(t *testing.T) {
	ds := testDaemonSet(2)
	other := daemonSetHostObj("host-other", true, map[string]string{testHostGroupLabel: dsOtherGroup})
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB), other)

	schedule(t, r, ds)

	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 2, dsHostB: 2})
	w := byHost[dsHostA][0]
	if w.Labels[workloadDaemonSetNameLabel] != testDaemonSetName {
		t.Fatalf("expected selector label on workload, got %v", w.Labels)
	}
	if w.Annotations[workloadDaemonSetNameAnnotation] != testDaemonSetName {
		t.Fatalf("expected stable-name annotation on workload, got %v", w.Annotations)
	}

	if ds.Status.Replicas == nil || ds.Status.Replicas.Expected != 4 {
		t.Fatalf("expected Replicas.Expected=4, got %+v", ds.Status.Replicas)
	}
	if ds.Status.DesiredHosts != 2 {
		t.Fatalf("expected DesiredHosts=2, got %d", ds.Status.DesiredHosts)
	}
	// Status reflects this pass's creates.
	if ds.Status.Replicas.Current != 4 {
		t.Fatalf("expected Replicas.Current=4 after the first pass, got %d", ds.Status.Replicas.Current)
	}
}

// A reconcile that runs before the cache holds the last pass's creates
// collides with them by name instead of starting a second set.
func TestDaemonSetSchedule_StaleCacheDoesNotDuplicate(t *testing.T) {
	ds := testDaemonSet(2)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))
	schedule(t, r, ds)

	template := ds.Spec.Template.DeepCopy()
	host := &daemonSetHost{}
	if err := r.syncHost(context.Background(), ds, template, template.Hash(), dsHostA, host, 2, map[string]bool{}); err != nil {
		t.Fatalf("syncHost with a stale view: %v", err)
	}

	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 2})
}

// The ReplicaSet and Deployment annotations name the workload on the host,
// so a template copied from another manifest must not carry them through.
func TestDaemonSetSchedule_StripsReservedAnnotations(t *testing.T) {
	ds := testDaemonSet(1)
	ds.Spec.Template.Annotations = map[string]string{
		workloadDeploymentNameAnnotation: "someone-else",
		workloadReplicaSetNameAnnotation: "someone-else-abc",
		"example.com/keep":               "yes",
	}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))
	schedule(t, r, ds)

	annotations := workloadsByHost(t, r)[dsHostA][0].Annotations
	if _, ok := annotations[workloadDeploymentNameAnnotation]; ok {
		t.Fatalf("workload-deployment annotation leaked onto the instance: %v", annotations)
	}
	if _, ok := annotations[workloadReplicaSetNameAnnotation]; ok {
		t.Fatalf("workload-replicaset-name annotation leaked onto the instance: %v", annotations)
	}
	if annotations["example.com/keep"] != "yes" || annotations[workloadDaemonSetNameAnnotation] != testDaemonSetName {
		t.Fatalf("expected user and daemonset annotations kept, got %v", annotations)
	}
}

// An instance whose Sync went False while its host was down is not failed
// until it stays down for the grace period after the host is Ready again.
func TestDaemonSetSchedule_HostBlipKeepsInstances(t *testing.T) {
	for _, tc := range []struct {
		name        string
		hostReadyAt time.Time
		wantKept    bool
	}{
		{name: "host just came back", hostReadyAt: time.Now(), wantKept: true},
		{name: "host back past the grace period", hostReadyAt: time.Now().Add(-10 * time.Minute), wantKept: false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ds := testDaemonSet(1)
			hostA := defaultGroupHost(dsHostA)
			hostA.Status.Conditions[0].LastTransitionTime = metav1.NewTime(tc.hostReadyAt)
			r := newDaemonSetReconciler(t, ds, hostA)
			schedule(t, r, ds)

			w := workloadsByHost(t, r)[dsHostA][0]
			sync := condition.ErrorCondition(runtimev1alpha1.WorkloadConditionSync, "Reconcile", errors.New("host unreachable"))
			sync.LastTransitionTime = metav1.NewTime(time.Now().Add(-5 * time.Minute))
			w.Status.Conditions = []condition.Condition{sync}
			if err := r.Update(context.Background(), &w); err != nil {
				t.Fatalf("update workload status: %v", err)
			}

			schedule(t, r, ds)

			kept := false
			for _, got := range workloadsByHost(t, r)[dsHostA] {
				if got.Name == w.Name {
					kept = true
				}
			}
			if kept != tc.wantKept {
				t.Fatalf("expected kept=%v for %s", tc.wantKept, w.Name)
			}
		})
	}
}

func TestDaemonSetsForHost_OnlyMatchingDaemonSets(t *testing.T) {
	matching := testDaemonSet(1)
	other := testDaemonSet(1)
	other.Name = "elsewhere"
	other.UID = "other-uid"
	other.Spec.Template.Spec.HostSelector = map[string]string{testHostGroupLabel: dsOtherGroup}
	isolated := testDaemonSet(1)
	isolated.Name = "isolated"
	isolated.UID = "isolated-uid"
	isolated.Spec.Template.Spec.Environment = testDaemonSetNamespace
	r := newDaemonSetReconciler(t, matching, other, isolated)

	host := defaultGroupHost(dsHostA)
	host.Environment = "tenant-b"
	requests := r.daemonSetsForHost(context.Background(), host)

	if len(requests) != 1 || requests[0].Name != testDaemonSetName {
		t.Fatalf("expected only %s enqueued, got %v", testDaemonSetName, requests)
	}
}

func TestDaemonSetSchedule_NewHostGetsInstances(t *testing.T) {
	ds := testDaemonSet(2)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))
	schedule(t, r, ds)
	before := workloadsByHost(t, r)[dsHostA]

	if err := r.Create(context.Background(), defaultGroupHost(dsHostB)); err != nil {
		t.Fatalf("create host: %v", err)
	}
	schedule(t, r, ds)

	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 2, dsHostB: 2})
	for i, w := range byHost[dsHostA] {
		if w.Name != before[i].Name {
			t.Fatalf("existing host's workloads were replaced: %s -> %s", before[i].Name, w.Name)
		}
	}
}

func TestDaemonSetSchedule_RemovedHostIsNotRescheduled(t *testing.T) {
	ds := testDaemonSet(2)
	hostB := defaultGroupHost(dsHostB)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), hostB)
	schedule(t, r, ds)

	if err := r.Delete(context.Background(), hostB); err != nil {
		t.Fatalf("delete host: %v", err)
	}
	schedule(t, r, ds)

	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 2})
}

func TestDaemonSetSchedule_HostNoLongerMatchingLosesInstances(t *testing.T) {
	ds := testDaemonSet(1)
	hostB := defaultGroupHost(dsHostB)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), hostB)
	schedule(t, r, ds)

	hostB.Labels = map[string]string{testHostGroupLabel: dsOtherGroup}
	if err := r.Update(context.Background(), hostB); err != nil {
		t.Fatalf("relabel host: %v", err)
	}
	schedule(t, r, ds)

	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
}

// An unready host gets nothing new, and keeps what it runs: the host
// controller deletes it, and with it those instances, if it stays down.
func TestDaemonSetSchedule_UnreadyHost(t *testing.T) {
	ds := testDaemonSet(1)
	hostB := defaultGroupHost(dsHostB)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), hostB,
		daemonSetHostObj(dsHostC, false, map[string]string{testHostGroupLabel: testHostGroup}))
	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1, dsHostB: 1})

	hostB.Status.SetConditions(condition.UnknownCondition(condition.TypeReady, "Reconcile", "heartbeat missed"))
	if err := r.Update(context.Background(), hostB); err != nil {
		t.Fatalf("update host status: %v", err)
	}
	schedule(t, r, ds)

	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1, dsHostB: 1})
	if ds.Status.DesiredHosts != 1 {
		t.Fatalf("expected DesiredHosts=1 with one ready host, got %d", ds.Status.DesiredHosts)
	}
}

func TestDaemonSetSchedule_ScaleDownPerHost(t *testing.T) {
	ds := testDaemonSet(3)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB))
	schedule(t, r, ds)

	one := int32(1)
	ds.Spec.Replicas = &one
	schedule(t, r, ds)

	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1, dsHostB: 1})
}

func TestDaemonSetSchedule_RollingUpdateOneHostAtATime(t *testing.T) {
	ds := testDaemonSet(1)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB))
	schedule(t, r, ds)
	markAllAvailable(t, r)

	ds.Spec.Template.Spec.Components[0].Image = "echo:v2"
	newHash := ds.Spec.Template.Hash()
	countNew := func(workloads []runtimev1alpha1.Workload) int {
		n := 0
		for _, w := range workloads {
			if w.Annotations[workloadDaemonSetGenerationAnnotation] == newHash {
				n++
			}
		}
		return n
	}

	// First pass: only ds-host-a starts the new template; nothing old stops yet.
	schedule(t, r, ds)
	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 2, dsHostB: 1})
	if countNew(byHost[dsHostA]) != 1 || countNew(byHost[dsHostB]) != 0 {
		t.Fatalf("expected the new template on ds-host-a only, got %v", byHost)
	}

	// Still not available: ds-host-b keeps waiting, ds-host-a keeps its old instance.
	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 2, dsHostB: 1})

	// The new instance is up: ds-host-a retires its old one, ds-host-b starts.
	markAllAvailable(t, r)
	schedule(t, r, ds)
	byHost = workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1, dsHostB: 2})
	if countNew(byHost[dsHostA]) != 1 || countNew(byHost[dsHostB]) != 1 {
		t.Fatalf("expected ds-host-a updated and ds-host-b updating, got %v", byHost)
	}
	// The pass that retired ds-host-a's old instance already reports it gone.
	if ds.Status.Hosts[0].HostID != dsHostA || ds.Status.Hosts[0].Outdated != 0 {
		t.Fatalf("expected ds-host-a outdated=0 in the same pass, got %+v", ds.Status.Hosts)
	}

	markAllAvailable(t, r)
	schedule(t, r, ds)
	schedule(t, r, ds)
	byHost = workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1, dsHostB: 1})
	if ds.Status.UpdatedHosts != 2 || ds.Status.ReadyHosts != 2 {
		t.Fatalf("expected both hosts updated and ready, got %+v", ds.Status)
	}
	ds.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDaemonSetConditionSchedule))
	if err := r.reconcileReady(context.Background(), ds); err != nil {
		t.Fatalf("expected Ready after rollout, got %v", err)
	}
}

func TestDaemonSetSchedule_Recreate(t *testing.T) {
	ds := testDaemonSet(1)
	ds.Spec.DeployPolicy = runtimev1alpha1.WorkloadDeployPolicyRecreate
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB))
	schedule(t, r, ds)
	markAllAvailable(t, r)

	ds.Spec.Template.Spec.Components[0].Image = "echo:v2"
	schedule(t, r, ds)

	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1, dsHostB: 1})
	newHash := ds.Spec.Template.Hash()
	for hostID, workloads := range byHost {
		if workloads[0].Annotations[workloadDaemonSetGenerationAnnotation] != newHash {
			t.Fatalf("host %s still runs the old template", hostID)
		}
	}
}

func TestDaemonSetReady_WaitsForEveryHost(t *testing.T) {
	ds := testDaemonSet(2)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), defaultGroupHost(dsHostB))
	schedule(t, r, ds)
	ds.Status.SetConditions(condition.ReadyCondition(runtimev1alpha1.WorkloadDaemonSetConditionSchedule))

	if err := r.reconcileReady(context.Background(), ds); !condition.IsStatusUnknown(err) {
		t.Fatalf("expected Ready to wait for workloads, got %v", err)
	}

	markAllAvailable(t, r)
	schedule(t, r, ds)
	if err := r.reconcileReady(context.Background(), ds); err != nil {
		t.Fatalf("expected Ready, got %v", err)
	}
	if ds.Status.Replicas.Ready != 4 {
		t.Fatalf("expected Replicas.Ready=4, got %d", ds.Status.Replicas.Ready)
	}
}
