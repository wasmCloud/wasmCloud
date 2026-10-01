package runtime

import (
	"sync"
	"time"
)

const retiredHostRetention = time.Hour

// RetiredHosts tracks removed Hosts and Pods until a later insert prunes them.
type RetiredHosts struct {
	mu   sync.RWMutex
	ids  map[string]time.Time
	pods map[string]retiredPod
}

type retiredPod struct {
	condemnedAt time.Time
	retiredAt   time.Time
}

func (r *RetiredHosts) retire(hostID string) {
	if r == nil || hostID == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.ids == nil {
		r.ids = make(map[string]time.Time)
	}
	now := time.Now()
	r.prune(now)
	r.ids[hostID] = now
}

func (r *RetiredHosts) retirePod(podIP string, condemnedAt time.Time) {
	if r == nil || podIP == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.pods == nil {
		r.pods = make(map[string]retiredPod)
	}
	now := time.Now()
	r.prune(now)
	r.pods[podIP] = retiredPod{condemnedAt: condemnedAt, retiredAt: now}
}

func (r *RetiredHosts) prune(now time.Time) {
	for id, retiredAt := range r.ids {
		if now.Sub(retiredAt) >= retiredHostRetention {
			delete(r.ids, id)
		}
	}
	for ip, pod := range r.pods {
		if now.Sub(pod.retiredAt) >= retiredHostRetention {
			delete(r.pods, ip)
		}
	}
}

func (r *RetiredHosts) contains(hostID string) bool {
	if r == nil {
		return false
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	_, ok := r.ids[hostID]
	return ok
}

// fromRetiredPod rejects hosts not known to postdate Pod removal.
func (r *RetiredHosts) fromRetiredPod(podIP string, startedAt time.Time) bool {
	if r == nil {
		return false
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	pod, ok := r.pods[podIP]
	// A missing start time cannot distinguish a replacement from the old host.
	return ok && (startedAt.IsZero() || !startedAt.After(pod.condemnedAt))
}
