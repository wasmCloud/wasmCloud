package runtime

import (
	"k8s.io/client-go/tools/events"
)

// Names the tests share: a Workload's namespace and Environment, another
// tenant's, and the usual Workload names.
const (
	testTenant  = "tenant-a"
	otherTenant = "tenant-b"
	placedName  = "placed"
	pinnedName  = "pinned"
)

// drainEvents returns every event recorded so far.
func drainEvents(recorder *events.FakeRecorder) []string {
	var got []string
	for {
		select {
		case event := <-recorder.Events:
			got = append(got, event)
		default:
			return got
		}
	}
}
