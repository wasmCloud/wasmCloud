package condition

import (
	"testing"

	corev1 "k8s.io/api/core/v1"
)

// FuzzSetConditions verifies SetConditions never panics and upholds two
// invariants under arbitrary inputs:
//  1. GetCondition always returns the type that was set.
//  2. Calling SetConditions twice with the same Type never grows the slice
//     beyond one entry for that type (dedup).
func FuzzSetConditions(f *testing.F) {
	f.Add("Ready", "True", "Available", "all good", "False")
	f.Add("", "", "", "", "")
	f.Add("Sync", "False", "ReconcileError", "something went wrong", "True")
	f.Add("HostSelection", "Unknown", "", "", "True")
	f.Add("很长的类型名", "True", "Reason", "message with unicode 🎉", "Unknown")

	f.Fuzz(func(t *testing.T, condType, status1, reason, message, status2 string) {
		s := &ConditionedStatus{}

		s.SetConditions(Condition{
			Type:    ConditionType(condType),
			Status:  corev1.ConditionStatus(status1),
			Reason:  ConditionReason(reason),
			Message: message,
		})

		// Invariant 1: GetCondition returns the type we set.
		got := s.GetCondition(ConditionType(condType))
		if got.Type != ConditionType(condType) {
			t.Errorf("GetCondition returned type %q, want %q", got.Type, condType)
		}
		if len(s.Conditions) != 1 {
			t.Errorf("expected 1 condition after first SetConditions, got %d", len(s.Conditions))
		}

		// Invariant 2: a second call with the same type must not grow the slice.
		s.SetConditions(Condition{
			Type:   ConditionType(condType),
			Status: corev1.ConditionStatus(status2),
		})
		count := 0
		for _, c := range s.Conditions {
			if c.Type == ConditionType(condType) {
				count++
			}
		}
		if count > 1 {
			t.Errorf("found %d entries for type %q after second SetConditions, want at most 1", count, condType)
		}
	})
}

// FuzzAllTrue verifies AllTrue and ErrAllTrue never panic and stay consistent
// with each other across arbitrary condition types, statuses, and
// ObservedGenerations.
//
// The generation seeds specifically exercise the mismatch branch in ErrAllTrue:
// two True conditions with different non-zero ObservedGenerations must not be
// considered all-true, even though both statuses are True.
func FuzzAllTrue(f *testing.F) {
	f.Add("Ready", "True", int64(0), "Sync", "True", int64(0))
	f.Add("Ready", "False", int64(0), "Sync", "Unknown", int64(0))
	f.Add("", "", int64(0), "", "", int64(0))
	// Matching non-zero generations — should be AllTrue.
	f.Add("Ready", "True", int64(5), "Sync", "True", int64(5))
	// Mismatched non-zero generations — ErrAllTrue must return an error even
	// though both statuses are True.
	f.Add("Ready", "True", int64(1), "Sync", "True", int64(2))
	// One condition tracks generation, the other does not (0 means untracked).
	f.Add("Ready", "True", int64(3), "Sync", "True", int64(0))

	f.Fuzz(func(t *testing.T, ct1, st1 string, gen1 int64, ct2, st2 string, gen2 int64) {
		s := &ConditionedStatus{}
		s.SetConditions(
			Condition{
				Type:               ConditionType(ct1),
				Status:             corev1.ConditionStatus(st1),
				ObservedGeneration: gen1,
			},
			Condition{
				Type:               ConditionType(ct2),
				Status:             corev1.ConditionStatus(st2),
				ObservedGeneration: gen2,
			},
		)

		all := s.AllTrue(ConditionType(ct1), ConditionType(ct2))
		err := s.ErrAllTrue(ConditionType(ct1), ConditionType(ct2))

		// AllTrue and ErrAllTrue must agree.
		if all && err != nil {
			t.Errorf("AllTrue=true but ErrAllTrue=%v", err)
		}
		if !all && err == nil {
			t.Error("AllTrue=false but ErrAllTrue=nil")
		}

		// Two True conditions with distinct non-zero generations must not pass.
		if ct1 != ct2 &&
			corev1.ConditionStatus(st1) == ConditionTrue && gen1 > 0 &&
			corev1.ConditionStatus(st2) == ConditionTrue && gen2 > 0 &&
			gen1 != gen2 {
			if all {
				t.Errorf("AllTrue=true for mismatched generations %d vs %d", gen1, gen2)
			}
		}
	})
}

// FuzzAnyUnknown verifies AnyUnknown never panics and is consistent with
// GetCondition: if AnyUnknown returns true, at least one of the queried
// conditions must have Unknown status.
func FuzzAnyUnknown(f *testing.F) {
	f.Add("Ready", "Unknown", "Sync", "True")
	f.Add("", "", "", "")
	f.Add("X", "False", "Y", "Unknown")

	f.Fuzz(func(t *testing.T, ct1, st1, ct2, st2 string) {
		s := &ConditionedStatus{}
		s.SetConditions(
			Condition{Type: ConditionType(ct1), Status: corev1.ConditionStatus(st1)},
			Condition{Type: ConditionType(ct2), Status: corev1.ConditionStatus(st2)},
		)

		if s.AnyUnknown(ConditionType(ct1), ConditionType(ct2)) {
			c1 := s.GetCondition(ConditionType(ct1))
			c2 := s.GetCondition(ConditionType(ct2))
			if c1.Status != ConditionUnknown && c2.Status != ConditionUnknown {
				t.Error("AnyUnknown=true but neither condition has Unknown status")
			}
		}
	})
}

// FuzzConditionedStatusEqual verifies Equal is reflexive, symmetric, and
// correctly distinguishes statuses built from different inputs.
func FuzzConditionedStatusEqual(f *testing.F) {
	// Identical inputs → Equal must be true.
	f.Add("Ready", "True", "reason", "msg", "Ready", "True", "reason", "msg")
	f.Add("", "", "", "", "", "", "", "")
	// Differing status → Equal must be false.
	f.Add("Ready", "True", "reason", "msg", "Ready", "False", "reason", "msg")
	// Differing type → Equal must be false.
	f.Add("Ready", "True", "reason", "msg", "Sync", "True", "reason", "msg")

	f.Fuzz(func(t *testing.T, ct1, st1, reason1, msg1, ct2, st2, reason2, msg2 string) {
		c1 := Condition{
			Type:    ConditionType(ct1),
			Status:  corev1.ConditionStatus(st1),
			Reason:  ConditionReason(reason1),
			Message: msg1,
		}
		c2 := Condition{
			Type:    ConditionType(ct2),
			Status:  corev1.ConditionStatus(st2),
			Reason:  ConditionReason(reason2),
			Message: msg2,
		}

		s1 := NewConditionedStatus(c1)
		s2 := NewConditionedStatus(c2)

		if !s1.Equal(s1) {
			t.Error("Equal is not reflexive")
		}
		if s1.Equal(s2) != s2.Equal(s1) {
			t.Error("Equal is not symmetric")
		}
		if c1.Equal(c2) && !s1.Equal(s2) {
			t.Error("conditions are Equal but ConditionedStatus.Equal returned false")
		}
		if ct1 != ct2 && s1.Equal(s2) {
			t.Errorf("statuses with different condition types %q vs %q reported Equal", ct1, ct2)
		}
	})
}
