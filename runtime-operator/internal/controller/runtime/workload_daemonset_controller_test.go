package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime/schema"
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
			ReplicasPerHost: &replicas,
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
		APIReader:         c,
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

// staleCacheClient reads like an informer cache that holds no Workloads yet.
type staleCacheClient struct {
	client.Client
}

func (c staleCacheClient) Get(ctx context.Context, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
	if _, ok := obj.(*runtimev1alpha1.Workload); ok {
		return apierrors.NewNotFound(schema.GroupResource{Group: "runtime.wasmcloud.dev", Resource: "workloads"}, key.Name)
	}
	return c.Client.Get(ctx, key, obj, opts...)
}

// A reconcile that runs before the cache holds the last pass's creates
// collides with them by name instead of starting a second set, and the
// collision is not an error even though the cache can't find them either.
func TestDaemonSetSchedule_StaleCacheDoesNotDuplicate(t *testing.T) {
	ds := testDaemonSet(2)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))
	schedule(t, r, ds)
	r.Client = staleCacheClient{Client: r.Client}

	template := ds.Spec.Template.DeepCopy()
	host := &daemonSetHost{}
	if err := r.syncHost(context.Background(), ds, template, template.Hash(), dsHostA, host, 2, map[string]bool{}); err != nil {
		t.Fatalf("syncHost with a stale view: %v", err)
	}

	if len(host.current) != 2 {
		t.Fatalf("expected the 2 existing instances counted, got %d", len(host.current))
	}
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 2})
}

// orphanedWorkload is what --cascade=orphan leaves: an instance with the
// DaemonSet's label and no controller.
func orphanedWorkload(name, hostID, generation string) *runtimev1alpha1.Workload {
	return &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{
			Namespace:   testDaemonSetNamespace,
			Name:        name,
			Labels:      map[string]string{workloadDaemonSetNameLabel: testDaemonSetName},
			Annotations: map[string]string{workloadDaemonSetGenerationAnnotation: generation},
		},
		Spec: runtimev1alpha1.WorkloadSpec{HostID: hostID},
	}
}

// otherControllerRef makes a Workload belong to something other than the DaemonSet.
func otherControllerRef() metav1.OwnerReference {
	controller := true
	return metav1.OwnerReference{
		APIVersion: runtimev1alpha1.GroupVersion.String(),
		Kind:       "WorkloadReplicaSet",
		Name:       "another-replicaset",
		UID:        "another-uid",
		Controller: &controller,
	}
}

func TestDaemonSetSchedule_AdoptsOrphanInSlot(t *testing.T) {
	ds := testDaemonSet(1)
	hash := ds.Spec.Template.Hash()
	orphan := orphanedWorkload(daemonSetWorkloadName(testDaemonSetName, dsHostA, hash, 0), dsHostA, hash)
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphan)

	schedule(t, r, ds)

	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1})
	if byHost[dsHostA][0].Name != orphan.Name {
		t.Fatalf("expected the orphan adopted, got %s", byHost[dsHostA][0].Name)
	}
}

// An orphan of an older template has a name no create collides with. It
// rolls like any other outdated instance.
func TestDaemonSetSchedule_OrphanOfOldTemplateRolls(t *testing.T) {
	ds := testDaemonSet(1)
	orphan := orphanedWorkload("echo-everywhere-old-0", dsHostA, "old-template")
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphan)

	schedule(t, r, ds)
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 2})

	markAllAvailable(t, r)
	schedule(t, r, ds)

	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1})
	if byHost[dsHostA][0].Name == orphan.Name {
		t.Fatalf("expected the old-template orphan replaced, got %s", orphan.Name)
	}
}

// A re-apply with a lower replicasPerHost leaves orphans in slots it won't fill.
func TestDaemonSetSchedule_ExcessOrphansAreRemoved(t *testing.T) {
	ds := testDaemonSet(1)
	hash := ds.Spec.Template.Hash()
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA),
		orphanedWorkload(daemonSetWorkloadName(testDaemonSetName, dsHostA, hash, 0), dsHostA, hash),
		orphanedWorkload(daemonSetWorkloadName(testDaemonSetName, dsHostA, hash, 1), dsHostA, hash),
	)

	schedule(t, r, ds)

	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
	labelled := &runtimev1alpha1.WorkloadList{}
	if err := r.List(context.Background(), labelled, client.InNamespace(testDaemonSetNamespace)); err != nil {
		t.Fatalf("list workloads: %v", err)
	}
	if len(labelled.Items) != 1 {
		t.Fatalf("expected the excess orphan removed, got %d workloads", len(labelled.Items))
	}
}

func TestDaemonSetSchedule_OrphanOnLeftHostIsRemoved(t *testing.T) {
	ds := testDaemonSet(1)
	orphan := orphanedWorkload("echo-everywhere-gone-0", dsHostB, ds.Spec.Template.Hash())
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphan)

	schedule(t, r, ds)

	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
	err := r.Get(context.Background(), client.ObjectKeyFromObject(orphan), &runtimev1alpha1.Workload{})
	if !apierrors.IsNotFound(err) {
		t.Fatalf("expected the orphan on a departed host removed, got %v", err)
	}
}

// An orphan already being deleted isn't adopted, and its name stays taken.
func TestDaemonSetSchedule_TerminatingOrphanHoldsItsSlot(t *testing.T) {
	ds := testDaemonSet(1)
	hash := ds.Spec.Template.Hash()
	orphan := orphanedWorkload(daemonSetWorkloadName(testDaemonSetName, dsHostA, hash, 0), dsHostA, hash)
	now := metav1.Now()
	orphan.DeletionTimestamp = &now
	orphan.Finalizers = []string{"example.com/hold"}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphan)

	schedule(t, r, ds)

	byHost := workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1})
	if byHost[dsHostA][0].Name != daemonSetWorkloadName(testDaemonSetName, dsHostA, hash, 1) {
		t.Fatalf("expected the next slot used, got %s", byHost[dsHostA][0].Name)
	}
}

// Deleting with --cascade=orphan orphans the Workloads while the DaemonSet is
// still there, and the cached copy may not show the delete yet. Adopting them
// back, or starting more, would undo what the delete asked for.
func TestDaemonSetSchedule_DeletingDaemonSetLeavesOrphans(t *testing.T) {
	for _, tc := range []struct {
		name        string
		staleCached bool
	}{
		{name: "cached copy shows the delete"},
		{name: "cached copy predates the delete", staleCached: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ds := testDaemonSet(1)
			now := metav1.Now()
			ds.DeletionTimestamp = &now
			ds.Finalizers = []string{"orphan"}
			hash := ds.Spec.Template.Hash()
			orphan := orphanedWorkload(daemonSetWorkloadName(testDaemonSetName, dsHostA, hash, 0), dsHostA, hash)
			r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), orphan)

			seen := ds.DeepCopy()
			if tc.staleCached {
				seen.DeletionTimestamp = nil
			}
			if err := r.reconcileSchedule(context.Background(), seen); !errors.Is(err, condition.ErrNoop()) {
				t.Fatalf("expected a no-op, got %v", err)
			}

			got := &runtimev1alpha1.Workload{}
			if err := r.Get(context.Background(), client.ObjectKeyFromObject(orphan), got); err != nil {
				t.Fatalf("get orphan: %v", err)
			}
			if owner := metav1.GetControllerOf(got); owner != nil {
				t.Fatalf("expected the orphan left alone, got owner %s", owner.Name)
			}
			assertPerHost(t, workloadsByHost(t, r), map[string]int{})
		})
	}
}

// A foreground delete is waiting on these Workloads to go, so none may start.
func TestDaemonSetSchedule_DeletingDaemonSetStartsNothing(t *testing.T) {
	ds := testDaemonSet(1)
	now := metav1.Now()
	ds.DeletionTimestamp = &now
	ds.Finalizers = []string{"foregroundDeletion"}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA))

	if err := r.reconcileSchedule(context.Background(), ds); !errors.Is(err, condition.ErrNoop()) {
		t.Fatalf("expected a no-op, got %v", err)
	}
	assertPerHost(t, workloadsByHost(t, r), map[string]int{})
}

// Without the DaemonSet's label a Workload isn't an orphan of it, so one that
// happens to hold a slot's name is an error rather than taken over.
func TestDaemonSetSchedule_UnlabelledSlotHolderFails(t *testing.T) {
	ds := testDaemonSet(1)
	holder := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{
			Namespace: testDaemonSetNamespace,
			Name:      daemonSetWorkloadName(testDaemonSetName, dsHostA, ds.Spec.Template.Hash(), 0),
		},
		Spec: runtimev1alpha1.WorkloadSpec{HostID: dsHostA},
	}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), holder)

	if err := r.reconcileSchedule(context.Background(), ds); err == nil {
		t.Fatal("expected an error for a slot held by an unlabelled Workload")
	}
	got := &runtimev1alpha1.Workload{}
	if err := r.Get(context.Background(), client.ObjectKeyFromObject(holder), got); err != nil {
		t.Fatalf("get holder: %v", err)
	}
	if metav1.GetControllerOf(got) != nil {
		t.Fatal("expected the unlabelled Workload left alone")
	}
}

// A Workload another controller owns is not an orphan, even with the label.
func TestDaemonSetSchedule_LeavesLabelledWorkloadOfAnotherController(t *testing.T) {
	ds := testDaemonSet(1)
	other := orphanedWorkload("echo-everywhere-other-0", dsHostA, ds.Spec.Template.Hash())
	other.OwnerReferences = []metav1.OwnerReference{otherControllerRef()}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), other)

	schedule(t, r, ds)

	got := &runtimev1alpha1.Workload{}
	if err := r.Get(context.Background(), client.ObjectKeyFromObject(other), got); err != nil {
		t.Fatalf("get workload: %v", err)
	}
	if owner := metav1.GetControllerOf(got); owner == nil || owner.Name != otherControllerRef().Name {
		t.Fatalf("expected its owner kept, got %v", owner)
	}
	assertPerHost(t, workloadsByHost(t, r), map[string]int{dsHostA: 1})
}

func TestDaemonSetSchedule_SlotOwnedByAnotherControllerFails(t *testing.T) {
	ds := testDaemonSet(1)
	foreign := &runtimev1alpha1.Workload{
		ObjectMeta: metav1.ObjectMeta{
			Namespace:       testDaemonSetNamespace,
			Name:            daemonSetWorkloadName(testDaemonSetName, dsHostA, ds.Spec.Template.Hash(), 0),
			OwnerReferences: []metav1.OwnerReference{otherControllerRef()},
		},
	}
	r := newDaemonSetReconciler(t, ds, defaultGroupHost(dsHostA), foreign)

	if err := r.reconcileSchedule(context.Background(), ds); err == nil {
		t.Fatal("expected an error for a slot another controller owns")
	}
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
	ds.Spec.ReplicasPerHost = &one
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
	// The pass that retired ds-host-a's old instance already reports it
	// done, so only ds-host-b is listed as needing attention.
	if len(ds.Status.Hosts) != 1 || ds.Status.Hosts[0].HostID != dsHostB {
		t.Fatalf("expected only ds-host-b in status.hosts, got %+v", ds.Status.Hosts)
	}

	markAllAvailable(t, r)
	schedule(t, r, ds)
	schedule(t, r, ds)
	byHost = workloadsByHost(t, r)
	assertPerHost(t, byHost, map[string]int{dsHostA: 1, dsHostB: 1})
	if ds.Status.UpdatedHosts != 2 || ds.Status.ReadyHosts != 2 {
		t.Fatalf("expected both hosts updated and ready, got %+v", ds.Status)
	}
	if len(ds.Status.Hosts) != 0 {
		t.Fatalf("expected no hosts needing attention, got %+v", ds.Status.Hosts)
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
