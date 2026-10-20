package runtime

import (
	"k8s.io/client-go/tools/events"
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
