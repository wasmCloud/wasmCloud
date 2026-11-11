package main

import (
	"testing"
)

func TestRunRejectsInvalidScalingBounds(t *testing.T) {
	for _, arg := range []string{
		"--workload-concurrency=0", "--workload-cleanup-concurrency=-1",
		"--workload-host-concurrency=0", "--replica-batch-size=0",
		"--replica-concurrency=-1", "--ready-requeue-interval=-1s",
	} {
		t.Run(arg, func(t *testing.T) {
			if err := runWith(t.Context(), []string{arg}, offlineConfig); err == nil {
				t.Fatal("accepted invalid scaling bounds")
			}
		})
	}
}
