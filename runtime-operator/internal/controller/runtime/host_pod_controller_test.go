package runtime

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/go-logr/logr"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/types/known/timestamppb"
	corev1 "k8s.io/api/core/v1"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"
	"sigs.k8s.io/controller-runtime/pkg/client/interceptor"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
	runtimev2 "go.wasmcloud.dev/runtime-operator/v2/pkg/rpc/wasmcloud/runtime/v2"
)

func heartbeatBytesAt(t *testing.T, name, hostID, podIP string, startedAt time.Time) []byte {
	t.Helper()
	data, err := protojson.Marshal(&runtimev2.HostHeartbeat{
		FriendlyName: name,
		Id:           hostID,
		Hostname:     podIP,
		StartedAt:    timestamppb.New(startedAt),
	})
	if err != nil {
		t.Fatalf("marshal heartbeat: %v", err)
	}
	return data
}

// newHostPodClient builds a fake client wired with the same Hostname index
// deleteHostForPod relies on in production, seeded with the given objects.
func newHostPodClient(t *testing.T, objs ...client.Object) client.Client {
	t.Helper()
	return hostPodClientBuilder(t, objs...).Build()
}

func hostPodClientBuilder(t *testing.T, objs ...client.Object) *fake.ClientBuilder {
	t.Helper()
	s := runtime.NewScheme()
	if err := runtimev1alpha1.AddToScheme(s); err != nil {
		t.Fatalf("add runtime v1alpha1: %v", err)
	}
	if err := corev1.AddToScheme(s); err != nil {
		t.Fatalf("add corev1: %v", err)
	}
	return fake.NewClientBuilder().
		WithScheme(s).
		WithObjects(objs...).
		WithIndex(&runtimev1alpha1.Host{}, hostnameFieldIndex,
			func(obj client.Object) []string {
				host, ok := obj.(*runtimev1alpha1.Host)
				if !ok || host.Hostname == "" {
					return nil
				}
				return []string{host.Hostname}
			}).
		WithIndex(&runtimev1alpha1.Host{}, hostIDIndex,
			func(obj client.Object) []string {
				host, ok := obj.(*runtimev1alpha1.Host)
				if !ok || host.HostID == "" {
					return nil
				}
				return []string{host.HostID}
			}).
		WithIndex(&corev1.Pod{}, hostPodIPFieldIndex, hostPodIPIndexValue)
}

func livePod(name, podIP string) *corev1.Pod {
	return &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: testNamespace,
			Labels:    map[string]string{HostPodLabel: "pool-a"},
		},
		Status: corev1.PodStatus{PodIP: podIP},
	}
}

func TestHostPodDraining(t *testing.T) {
	const podIP = "10.1.2.3"
	for name, tc := range map[string]struct {
		pods []client.Object
		want bool
	}{
		"no pod holds the IP":            {want: false},
		"its only pod is terminating":    {pods: []client.Object{terminatingPod(podIP, time.Now(), 30)}, want: true},
		"a live pod has the recycled IP": {pods: []client.Object{terminatingPod(podIP, time.Now(), 30), livePod(testReplacement, podIP)}, want: false},
		"its pod is live":                {pods: []client.Object{livePod("live", podIP)}, want: false},
	} {
		got, err := hostPodDraining(context.Background(), newHostPodClient(t, tc.pods...), podIP)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if got != tc.want {
			t.Errorf("%s: draining = %v, want %v", name, got, tc.want)
		}
	}
}

// A host keeps heartbeating while its Pod drains. Once its Host is deleted, a
// heartbeat must not write it back, or the draining host takes new Workloads.
func TestHeartbeatHandler_SkipsDrainingHostPod(t *testing.T) {
	const podIP = "10.1.2.3"
	for _, draining := range []bool{true, false} {
		pod := livePod("host-pod", podIP)
		if draining {
			pod = terminatingPod(podIP, time.Now(), 30)
		}
		writes := 0
		countWrites := interceptor.Funcs{
			Patch: func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
				writes++
				return nil
			},
			SubResourcePatch: func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
				writes++
				return nil
			},
		}
		updater := &hostStatusUpdater{
			client:            hostPodClientBuilder(t, pod).WithInterceptorFuncs(countWrites).Build(),
			operatorNamespace: testNamespace,
			fleet:             &fleetWitness{},
		}

		updater.handleHeartbeat(context.Background(), logr.Discard(),
			"runtime.operator.heartbeat."+testHeartbeatHostID, heartbeatBytes(t, "hb-host", testHeartbeatHostID, podIP))

		if draining && writes != 0 {
			t.Errorf("a draining host's heartbeat wrote its Host back %d times", writes)
		}
		if !draining && writes == 0 {
			t.Errorf("a live host's heartbeat did not register its Host")
		}
	}
}

func TestHeartbeatHandler_SkipsRetiredHostAfterPodIsGone(t *testing.T) {
	const podIP = "10.1.2.3"
	condemnedAt := time.Now()
	host := hostAt("hb-host", podIP, condemnedAt.Add(-time.Minute))
	host.HostID = testHeartbeatHostID
	retired := &RetiredHosts{}
	writes := 0
	c := hostPodClientBuilder(t, host).WithInterceptorFuncs(interceptor.Funcs{
		Patch: func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
			writes++
			return nil
		},
		SubResourcePatch: func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
			writes++
			return nil
		},
	}).Build()
	r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace, RetiredHosts: retired}
	if err := r.deleteHostForPod(context.Background(), terminatingPod(podIP, condemnedAt, 0)); err != nil {
		t.Fatalf("deleteHostForPod: %v", err)
	}
	if !retired.contains(testHeartbeatHostID) {
		t.Fatal("deleted Host ID was not retired")
	}

	updater := &hostStatusUpdater{client: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}
	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+testHeartbeatHostID, heartbeatBytes(t, "hb-host", testHeartbeatHostID, podIP))
	if writes != 0 {
		t.Errorf("retired host was recreated after its Pod disappeared: %d writes", writes)
	}

	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+"replacement-id", heartbeatBytesAt(t, testReplacement, "replacement-id", podIP, condemnedAt.Add(time.Second)))
	if writes == 0 {
		t.Error("a new Host ID on the recycled IP was not registered")
	}
}

// retiredPodIP is the Pod IP of the Host that the retirement tests retire.
const retiredPodIP = "10.1.2.3"

// retiringHost is a Host under testHeartbeatHostID registered at retiredPodIP
// well before its Pod was condemned.
func retiringHost(condemnedAt time.Time) *runtimev1alpha1.Host {
	host := hostAt("old-host", retiredPodIP, condemnedAt.Add(-time.Hour))
	host.HostID = testHeartbeatHostID
	host.UID = "old-host-uid"
	return host
}

// retireHostAt runs the finalizer of the Pod at retiredPodIP against c.
func retireHostAt(t *testing.T, c client.Client, condemnedAt time.Time, retired *RetiredHosts) {
	t.Helper()
	r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace, RetiredHosts: retired}
	if err := r.deleteHostForPod(context.Background(), terminatingPod(retiredPodIP, condemnedAt, 30)); err != nil {
		t.Fatalf("deleteHostForPod: %v", err)
	}
	if !retired.contains(testHeartbeatHostID) {
		t.Fatal("the deleted Host's ID was not retired")
	}
}

func hostsWithID(t *testing.T, c client.Client) []runtimev1alpha1.Host {
	t.Helper()
	hosts := &runtimev1alpha1.HostList{}
	if err := c.List(context.Background(), hosts, client.InNamespace(testNamespace),
		client.MatchingFields{hostIDIndex: testHeartbeatHostID}); err != nil {
		t.Fatal(err)
	}
	return hosts.Items
}

func sendHeartbeat(updater *hostStatusUpdater, heartbeat []byte) {
	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+testHeartbeatHostID, heartbeat)
}

// countingReader counts reads past the cache.
type countingReader struct {
	client.Reader
	gets int
}

func (r *countingReader) Get(ctx context.Context, key client.ObjectKey, obj client.Object, opts ...client.GetOption) error {
	r.gets++
	return r.Reader.Get(ctx, key, obj, opts...)
}

// A replacement at another Pod IP that started before the old Pod was condemned,
// as in an overlapping restart, registers; the old host stays refused.
func TestHeartbeatHandler_RetiredIDReplacementThatStartedFirst(t *testing.T) {
	const newIP = "10.1.2.4"
	condemnedAt := time.Now()
	c := hostPodClientBuilder(t, retiringHost(condemnedAt)).Build()
	retired := &RetiredHosts{}
	retireHostAt(t, c, condemnedAt, retired)
	updater := &hostStatusUpdater{client: c, apiReader: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}

	sendHeartbeat(updater, heartbeatBytesAt(t, testReplacement, testHeartbeatHostID, newIP, condemnedAt.Add(-30*time.Second)))
	sendHeartbeat(updater, heartbeatBytesAt(t, "old-host", testHeartbeatHostID, retiredPodIP, condemnedAt.Add(-time.Hour)))

	hosts := hostsWithID(t, c)
	if len(hosts) != 1 || hosts[0].Hostname != newIP {
		got := make([]string, 0, len(hosts))
		for _, host := range hosts {
			got = append(got, host.Hostname)
		}
		t.Fatalf("Hosts under the retired ID are at %v, want only the replacement at %s", got, newIP)
	}
}

// At the retired Pod IP, a host started after the Pod was condemned is a replacement, even
// if the retirement was recorded later; one started before, or with no start time, is not.
func TestHeartbeatHandler_RetiredIDOnARecycledIP(t *testing.T) {
	condemnedAt := time.Now().Add(-time.Minute)
	c := hostPodClientBuilder(t, retiringHost(condemnedAt)).Build()
	retired := &RetiredHosts{}
	retireHostAt(t, c, condemnedAt, retired)
	updater := &hostStatusUpdater{client: c, apiReader: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}

	sendHeartbeat(updater, heartbeatBytes(t, "old-host", testHeartbeatHostID, retiredPodIP))
	sendHeartbeat(updater, heartbeatBytesAt(t, "old-host", testHeartbeatHostID, retiredPodIP, condemnedAt.Add(-time.Hour)))
	if n := len(hostsWithID(t, c)); n != 0 {
		t.Fatalf("the retired host wrote %d Hosts back", n)
	}

	sendHeartbeat(updater, heartbeatBytesAt(t, testReplacement, testHeartbeatHostID, retiredPodIP, condemnedAt.Add(time.Second)))
	hosts := hostsWithID(t, c)
	if len(hosts) != 1 || hosts[0].Name != testReplacement {
		t.Fatalf("%d Hosts under the retired ID, want the replacement that started after the Pod was condemned", len(hosts))
	}
}

// Right after the Delete the cache can still show the retired Host, so its
// replacement waits until a read past the cache finds that Host gone.
func TestHeartbeatHandler_RetiredIDReplacementWaitsPastAStaleCache(t *testing.T) {
	const newIP = "10.1.2.4"
	condemnedAt := time.Now()
	old := retiringHost(condemnedAt)
	old.Finalizers = []string{hostFinalizerName}
	api := hostPodClientBuilder(t, old.DeepCopy()).Build()
	retired := &RetiredHosts{}
	retireHostAt(t, api, condemnedAt, retired)
	stale := hostPodClientBuilder(t, old.DeepCopy()).Build()
	updater := &hostStatusUpdater{client: stale, apiReader: api, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}
	heartbeat := heartbeatBytesAt(t, testReplacement, testHeartbeatHostID, newIP, time.Now())
	replacement := client.ObjectKey{Namespace: testNamespace, Name: testReplacement}

	sendHeartbeat(updater, heartbeat)
	if err := stale.Get(context.Background(), replacement, &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
		t.Fatalf("the replacement registered while the retired Host was still being deleted (err %v)", err)
	}

	cleaned := &runtimev1alpha1.Host{}
	if err := api.Get(context.Background(), client.ObjectKeyFromObject(old), cleaned); err != nil {
		t.Fatal(err)
	}
	cleaned.Finalizers = nil
	if err := api.Update(context.Background(), cleaned); err != nil {
		t.Fatal(err)
	}
	sendHeartbeat(updater, heartbeat)
	if err := stale.Get(context.Background(), replacement, &runtimev1alpha1.Host{}); err != nil {
		t.Fatalf("the replacement did not register once the retired Host was gone: %v", err)
	}
}

// A Pod finalizer run again from a stale cache can retire a key that a same-named
// successor now holds; another object at the name ends the wait.
func TestHeartbeatHandler_RetiredIDWaitEndsWhenTheNameHoldsAnotherHost(t *testing.T) {
	const newIP = "10.1.2.4"
	condemnedAt := time.Now()
	old := retiringHost(condemnedAt)
	retired := &RetiredHosts{}
	retired.retire(old, condemnedAt)
	successor := hostAt(old.Name, newIP, condemnedAt)
	successor.HostID = testHeartbeatHostID
	successor.UID = "successor-uid"
	writes := 0
	c := hostPodClientBuilder(t, successor).WithInterceptorFuncs(interceptor.Funcs{
		SubResourcePatch: func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
			writes++
			return nil
		},
	}).Build()
	updater := &hostStatusUpdater{client: c, apiReader: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}

	sendHeartbeat(updater, heartbeatBytesAt(t, old.Name, testHeartbeatHostID, newIP, time.Now()))
	if writes == 0 {
		t.Error("the successor waited for a retired Host whose name now holds another object")
	}
}

// A failed read past the cache keeps the replacement waiting: the retired Host
// may still be there.
func TestHeartbeatHandler_RetiredIDWaitsWhenTheReadFails(t *testing.T) {
	condemnedAt := time.Now()
	c := hostPodClientBuilder(t, retiringHost(condemnedAt)).Build()
	retired := &RetiredHosts{}
	retireHostAt(t, c, condemnedAt, retired)
	failing := hostPodClientBuilder(t).WithInterceptorFuncs(interceptor.Funcs{
		Get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
			return errors.New("API server unavailable")
		},
	}).Build()
	updater := &hostStatusUpdater{client: c, apiReader: failing, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}

	sendHeartbeat(updater, heartbeatBytesAt(t, testReplacement, testHeartbeatHostID, "10.1.2.4", time.Now()))
	if n := len(hostsWithID(t, c)); n != 0 {
		t.Errorf("the replacement registered although the retired Host could not be read: %d Hosts", n)
	}
}

// Once a read finds the retired Host gone, the replacement's later heartbeats
// check nothing more about it while its ID stays retired.
func TestHeartbeatHandler_RetiredIDStopsCheckingOnceHostIsGone(t *testing.T) {
	const newIP = "10.1.2.4"
	condemnedAt := time.Now()
	hostLists := 0
	c := hostPodClientBuilder(t, retiringHost(condemnedAt)).WithInterceptorFuncs(interceptor.Funcs{
		List: func(ctx context.Context, c client.WithWatch, list client.ObjectList, opts ...client.ListOption) error {
			if _, ok := list.(*runtimev1alpha1.HostList); ok {
				hostLists++
			}
			return c.List(ctx, list, opts...)
		},
	}).Build()
	retired := &RetiredHosts{}
	retireHostAt(t, c, condemnedAt, retired)
	reader := &countingReader{Reader: c}
	updater := &hostStatusUpdater{client: c, apiReader: reader, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}
	heartbeat := heartbeatBytesAt(t, testReplacement, testHeartbeatHostID, newIP, time.Now())

	sendHeartbeat(updater, heartbeat)
	if n := len(hostsWithID(t, c)); n != 1 {
		t.Fatalf("%d Hosts under the retired ID, want the replacement", n)
	}
	hostLists, reader.gets = 0, 0
	for range 3 {
		sendHeartbeat(updater, heartbeat)
	}
	if hostLists != 0 || reader.gets != 0 {
		t.Errorf("a registered replacement still checked the retired Host: %d Host lists, %d reads past the cache", hostLists, reader.gets)
	}
}

func TestHeartbeatHandler_SkipsRetiredPodWhenHostWasAlreadyGone(t *testing.T) {
	const podIP = "10.1.2.3"
	condemnedAt := time.Now().Add(-time.Minute)
	retired := &RetiredHosts{}
	writes := 0
	c := hostPodClientBuilder(t).WithInterceptorFuncs(interceptor.Funcs{
		Patch: func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
			writes++
			return nil
		},
		SubResourcePatch: func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
			writes++
			return nil
		},
	}).Build()
	r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace, RetiredHosts: retired}
	if err := r.deleteHostForPod(context.Background(), terminatingPod(podIP, condemnedAt, 0)); err != nil {
		t.Fatalf("deleteHostForPod: %v", err)
	}

	updater := &hostStatusUpdater{client: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}
	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+"old-id", heartbeatBytesAt(t, "old", "old-id", podIP, condemnedAt.Add(-time.Minute)))
	if writes != 0 {
		t.Errorf("old host was recreated after its Pod disappeared: %d writes", writes)
	}
	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+"replacement-id", heartbeatBytesAt(t, testReplacement, "replacement-id", podIP, condemnedAt.Add(time.Second)))
	if writes == 0 {
		t.Error("replacement host on recycled IP was not registered")
	}
}

func TestHeartbeatHandler_RejectsRetiredIDBeforeHostRead(t *testing.T) {
	retired := &RetiredHosts{}
	retired.retire(&runtimev1alpha1.Host{HostID: testHeartbeatHostID, Hostname: "10.1.2.3"}, time.Now())
	reads := 0
	c := hostPodClientBuilder(t).WithInterceptorFuncs(interceptor.Funcs{
		Get: func(context.Context, client.WithWatch, client.ObjectKey, client.Object, ...client.GetOption) error {
			reads++
			return nil
		},
	}).Build()
	updater := &hostStatusUpdater{client: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}
	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+testHeartbeatHostID, heartbeatBytes(t, "old", testHeartbeatHostID, "10.1.2.3"))
	if reads != 0 {
		t.Errorf("retired Host ID reached a stale Host read: %d reads", reads)
	}
}

func TestHeartbeatHandler_RegistersUnknownHostWhenPodLookupFails(t *testing.T) {
	listErr := errors.New("pod cache unavailable")
	retired := &RetiredHosts{}
	retired.retire(&runtimev1alpha1.Host{HostID: "retired-id", Hostname: "10.1.2.3"}, time.Now())
	writes := 0
	c := hostPodClientBuilder(t).WithInterceptorFuncs(interceptor.Funcs{
		List: func(_ context.Context, _ client.WithWatch, list client.ObjectList, _ ...client.ListOption) error {
			if _, ok := list.(*corev1.PodList); ok {
				return listErr
			}
			return nil
		},
		Patch: func(context.Context, client.WithWatch, client.Object, client.Patch, ...client.PatchOption) error {
			writes++
			return nil
		},
		SubResourcePatch: func(context.Context, client.Client, string, client.Object, client.Patch, ...client.SubResourcePatchOption) error {
			writes++
			return nil
		},
	}).Build()
	updater := &hostStatusUpdater{client: c, operatorNamespace: testNamespace, fleet: &fleetWitness{}, retiredHosts: retired}
	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+"retired-id", heartbeatBytes(t, "retired", "retired-id", "10.1.2.3"))
	if writes != 0 {
		t.Errorf("retired host was registered despite failed Pod lookup: %d writes", writes)
	}
	updater.handleHeartbeat(context.Background(), logr.Discard(),
		"runtime.operator.heartbeat."+testHeartbeatHostID, heartbeatBytes(t, "hb-host", testHeartbeatHostID, "10.1.2.3"))
	if writes == 0 {
		t.Error("new host was not registered after Pod lookup failed")
	}
}

func hostAt(name, hostname string, created time.Time) *runtimev1alpha1.Host {
	return &runtimev1alpha1.Host{
		ObjectMeta: metav1.ObjectMeta{
			Name:              name,
			Namespace:         testNamespace,
			CreationTimestamp: metav1.NewTime(created),
		},
		HostID:   name + "-id",
		Hostname: hostname,
	}
}

// terminatingPod builds a Pod condemned at condemnedAt with the given grace
// period, mirroring how the API server records a delete: DeletionTimestamp is
// the grace *deadline*, condemnedAt + grace, not the moment of the request.
func terminatingPod(podIP string, condemnedAt time.Time, grace int64) *corev1.Pod {
	deadline := metav1.NewTime(condemnedAt.Add(time.Duration(grace) * time.Second))
	return &corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name:                       "hostgroup-a",
			Namespace:                  testNamespace,
			Labels:                     map[string]string{HostPodLabel: "pool-a"},
			DeletionTimestamp:          &deadline,
			DeletionGracePeriodSeconds: &grace,
			Finalizers:                 []string{podHostFinalizerName},
		},
		Status: corev1.PodStatus{PodIP: podIP},
	}
}

// TestDeleteHostForPod covers the Pod-IP-to-Host mapping the finalizer uses.
// Kubernetes recycles Pod IPs, and this controller's own finalizer holds the
// terminating Pod object in the API well past the point where its IP was
// released — so the Host registered under that IP may already belong to the
// replacement Pod, and deleting it would cascade into a live host's Workloads.
func TestDeleteHostForPod(t *testing.T) {
	const podIP = "10.1.2.3"
	condemnedAt := time.Now()

	t.Run("deletes the host this pod registered", func(t *testing.T) {
		host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, host)
		r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
			t.Errorf("the pod's own host should have been deleted, got err=%v", err)
		}
	})

	t.Run("spares a host registered after this pod was deleted", func(t *testing.T) {
		recycled := hostAt("replacement-host", podIP, condemnedAt.Add(2*time.Second))
		c := newHostPodClient(t, recycled)
		r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(recycled), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("a host that registered after the pod was deleted must be left alone, got err=%v", err)
		}
	})

	t.Run("spares a replacement registered inside the grace window", func(t *testing.T) {
		// A terminating pod gets a grace period — 30s on the Kubernetes
		// default, 15s for a host pod from the chart — so DeletionTimestamp
		// sits that far in the future, covering the live hosts a replacement
		// registered while this Pod wound down.
		recycled := hostAt("replacement-host", podIP, condemnedAt.Add(8*time.Second))
		c := newHostPodClient(t, recycled)
		r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 30)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(recycled), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("a host registered during the grace window belongs to the replacement pod, got err=%v", err)
		}
	})

	t.Run("still deletes its own host when a grace period is set", func(t *testing.T) {
		host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, host)
		r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 30)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
			t.Errorf("the pod's own host should still be deleted, got err=%v", err)
		}
	})

	t.Run("leaves hosts on other IPs alone", func(t *testing.T) {
		other := hostAt("other-host", "10.1.2.4", condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, other)
		r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace}

		pod := terminatingPod(podIP, condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(other), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("host on another IP must be untouched, got err=%v", err)
		}
	})

	t.Run("pod without an IP deletes nothing", func(t *testing.T) {
		host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
		c := newHostPodClient(t, host)
		r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace}

		pod := terminatingPod("", condemnedAt, 0)
		if err := r.deleteHostForPod(context.Background(), pod); err != nil {
			t.Fatalf("deleteHostForPod: %v", err)
		}
		if err := c.Get(context.Background(), client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); err != nil {
			t.Errorf("a pod with no IP must not delete any host, got err=%v", err)
		}
	})
}

// TestHostPodReconcile_RemovesFinalizerAfterCleanup walks the terminating-pod
// path end to end: the host is deleted and the finalizer released so
// Kubernetes can finish removing the Pod.
func TestHostPodReconcile_RemovesFinalizerAfterCleanup(t *testing.T) {
	const podIP = "10.1.2.3"
	condemnedAt := time.Now()

	host := hostAt("own-host", podIP, condemnedAt.Add(-10*time.Minute))
	pod := terminatingPod(podIP, condemnedAt, 0)
	c := newHostPodClient(t, host, pod)
	r := &HostPodReconciler{Client: c, apiReader: c, OperatorNamespace: testNamespace}

	ctx := context.Background()
	req := ctrl.Request{NamespacedName: client.ObjectKeyFromObject(pod)}
	if _, err := r.Reconcile(ctx, req); err != nil {
		t.Fatalf("Reconcile: %v", err)
	}

	if err := c.Get(ctx, client.ObjectKeyFromObject(host), &runtimev1alpha1.Host{}); !apierrors.IsNotFound(err) {
		t.Errorf("host should have been deleted, got err=%v", err)
	}
	// Releasing the last finalizer lets the fake client complete the deletion.
	if err := c.Get(ctx, client.ObjectKeyFromObject(pod), &corev1.Pod{}); !apierrors.IsNotFound(err) {
		t.Errorf("pod finalizer should have been removed, got err=%v", err)
	}
}
