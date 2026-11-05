package condition

import (
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

func populatedStatus() *ConditionedStatus {
	at := metav1.NewTime(time.Date(2026, 11, 4, 12, 0, 0, 0, time.UTC))
	return &ConditionedStatus{Conditions: []Condition{
		{Type: TypeReady, Status: corev1.ConditionTrue, ObservedGeneration: 3, LastTransitionTime: at, LastProbeTime: at,
			Reason: "Reconciled", Message: "ready"},
		{Type: TypeSynced, Status: corev1.ConditionFalse, ObservedGeneration: 2, LastTransitionTime: at,
			Reason: "Waiting", Message: "not yet"},
	}}
}

func TestConditionedStatusDeepCopySharesNothing(t *testing.T) {
	original := populatedStatus()
	copied := original.DeepCopy()
	if diff := cmp.Diff(original, copied); diff != "" {
		t.Fatalf("copy differs from the original (-original +copy):\n%s", diff)
	}

	copied.Conditions[0].Status = corev1.ConditionFalse
	copied.Conditions[0].LastTransitionTime = metav1.NewTime(copied.Conditions[0].LastTransitionTime.Add(time.Hour))
	copied.Conditions[1].Message = "changed"
	copied.Conditions = append(copied.Conditions, Condition{Type: "Extra"})
	if diff := cmp.Diff(populatedStatus(), original); diff != "" {
		t.Errorf("changing the copy changed the original (-want +got):\n%s", diff)
	}

	condition := original.Conditions[1].DeepCopy()
	condition.Reason = "changed"
	if original.Conditions[1].Reason != "Waiting" {
		t.Errorf("changing a copied Condition changed the original: %+v", original.Conditions[1])
	}
}

func TestDeepCopyKeepsNilAndEmptyApart(t *testing.T) {
	if (*Condition)(nil).DeepCopy() != nil {
		t.Error("DeepCopy of a nil Condition is not nil")
	}
	if (*ConditionedStatus)(nil).DeepCopy() != nil {
		t.Error("DeepCopy of a nil ConditionedStatus is not nil")
	}
	if (*ReconcilerContext)(nil).DeepCopy() != nil {
		t.Error("DeepCopy of a nil ReconcilerContext is not nil")
	}
	if got := (&ConditionedStatus{}).DeepCopy(); got.Conditions != nil {
		t.Errorf("unset Conditions copied as %#v, want nil", got.Conditions)
	}
	if got := (&ConditionedStatus{Conditions: []Condition{}}).DeepCopy(); got.Conditions == nil {
		t.Error("empty Conditions copied as nil")
	}
}

func TestReconcilerContextDeepCopy(t *testing.T) {
	original := &ReconcilerContext{ForceUpdate: true, ForceRequeue: true, OptimisticLock: true, ReconcileInterval: time.Minute}
	copied := original.DeepCopy()
	if *copied != *original {
		t.Fatalf("copy = %+v, want %+v", copied, original)
	}
	copied.ForceUpdate = false
	if !original.ForceUpdate {
		t.Error("changing the copy changed the original")
	}
}
