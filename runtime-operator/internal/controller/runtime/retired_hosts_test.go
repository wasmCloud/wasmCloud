package runtime

import (
	"testing"
	"time"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

func TestRetiredHostsPrunesOldIDs(t *testing.T) {
	old := time.Now().Add(-retiredHostRetention - time.Minute)
	recent := time.Now().Add(-time.Minute)
	retired := &RetiredHosts{
		ids: map[string]retiredHost{"old": {retiredAt: old}, "recent": {retiredAt: recent}},
		pods: map[string]retiredPod{
			"old-ip":    {condemnedAt: old, retiredAt: old},
			"recent-ip": {condemnedAt: recent, retiredAt: recent},
		},
	}
	retired.retire(&runtimev1alpha1.Host{HostID: "new"}, time.Now())
	if retired.contains("old") {
		t.Error("old Host ID was not pruned")
	}
	if !retired.contains("recent") || !retired.contains("new") {
		t.Error("recent Host IDs were pruned")
	}
	if retired.fromRetiredPod("old-ip", old) {
		t.Error("old Pod IP was not pruned")
	}
	if !retired.fromRetiredPod("recent-ip", recent) {
		t.Error("recent Pod IP was pruned")
	}
}

func TestRetiredIDTellsTheRetiredHostFromAReplacement(t *testing.T) {
	const podIP = "10.1.2.3"
	condemnedAt := time.Now().Add(-time.Minute)
	retired := &RetiredHosts{}
	retired.retire(&runtimev1alpha1.Host{HostID: "id", Hostname: podIP}, condemnedAt)
	for name, tc := range map[string]struct {
		hostname  string
		startedAt time.Time
		refused   bool
	}{
		"the retired host":                         {podIP, condemnedAt.Add(-time.Hour), true},
		"the retired host with no start time":      {podIP, time.Time{}, true},
		"a heartbeat with no hostname":             {"", condemnedAt.Add(-time.Hour), true},
		"a host that got the IP after it was free": {podIP, condemnedAt.Add(time.Second), false},
		"a host at another IP":                     {"10.1.2.4", condemnedAt.Add(-time.Hour), false},
	} {
		if got := retired.retiredID("id", tc.hostname, tc.startedAt); got != tc.refused {
			t.Errorf("%s: refused = %v, want %v", name, got, tc.refused)
		}
	}
	if retired.retiredID("other-id", podIP, time.Time{}) {
		t.Error("an ID that was never retired was refused")
	}
}
