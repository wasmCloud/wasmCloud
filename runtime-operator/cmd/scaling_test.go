package main

import (
	"strings"
	"testing"
)

func TestRunRejectsInvalidScalingBounds(t *testing.T) {
	for _, arg := range []string{
		"--workload-concurrency=0", "--workload-cleanup-concurrency=-1",
		"--workload-host-concurrency=0", "--replica-batch-size=0",
		"--replica-concurrency=-1", "--ready-requeue-interval=-1s",
	} {
		t.Run(arg, func(t *testing.T) {
			// Assert the scaling refusal itself: run also fails later for
			// unrelated missing configuration.
			err := runWith(t.Context(), []string{arg}, offlineConfig)
			if err == nil || !strings.Contains(err.Error(), "scaling concurrency and batch sizes must be positive") {
				t.Fatalf("accepted invalid scaling bounds: %v", err)
			}
		})
	}
}
