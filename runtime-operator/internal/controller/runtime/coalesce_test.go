package runtime

import (
	"testing"
	"testing/synctest"
	"time"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	"k8s.io/client-go/util/workqueue"
	"sigs.k8s.io/controller-runtime/pkg/event"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

func TestCoalescingDoesNotPostponeAContinuousEventStream(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		q := workqueue.NewTypedRateLimitingQueue(workqueue.DefaultTypedControllerRateLimiter[reconcile.Request]())
		defer q.ShutDown()
		key := reconcile.Request{NamespacedName: types.NamespacedName{Name: "parent"}}
		coalesced := coalescingQueue{q}
		coalesced.Add(key)
		synctest.Wait()
		for range 4 {
			time.Sleep(20 * time.Millisecond)
			coalesced.Add(key)
			synctest.Wait()
			if q.Len() != 0 {
				t.Fatal("event was not coalesced")
			}
		}
		time.Sleep(20 * time.Millisecond)
		synctest.Wait()
		if q.Len() != 1 {
			t.Fatal("continuous events postponed the original deadline")
		}
		got, shutdown := q.Get()
		if shutdown || got != key {
			t.Fatal("wrong parent enqueued")
		}
		q.Done(got)
	})
}

func TestChildPredicatePreservesDeletionAndReadiness(t *testing.T) {
	old := placedWorkload()
	next := old.DeepCopy()
	next.ResourceVersion = "new-version"
	next.Finalizers = []string{workloadFinalizerName}
	p := childChanges()
	if p.Update(event.UpdateEvent{ObjectOld: old, ObjectNew: next}) {
		t.Fatal("finalizer-only change enqueued parent")
	}
	now := metav1.Now()
	next.DeletionTimestamp = &now
	if !p.Update(event.UpdateEvent{ObjectOld: old, ObjectNew: next}) {
		t.Fatal("deletion was filtered out")
	}
	next = old.DeepCopy()
	next.Status.WorkloadID = "new-placement"
	if !p.Update(event.UpdateEvent{ObjectOld: old, ObjectNew: next}) {
		t.Fatal("placement status was filtered out")
	}
}
