package runtime

import (
	"testing"
	"time"
)

func TestRetiredHostsPrunesOldIDs(t *testing.T) {
	old := time.Now().Add(-retiredHostRetention - time.Minute)
	recent := time.Now().Add(-time.Minute)
	retired := &RetiredHosts{
		ids: map[string]time.Time{"old": old, "recent": recent},
		pods: map[string]retiredPod{
			"old-ip":    {condemnedAt: old, retiredAt: old},
			"recent-ip": {condemnedAt: recent, retiredAt: recent},
		},
	}
	retired.retire("new")
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
