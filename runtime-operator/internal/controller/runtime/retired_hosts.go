package runtime

import (
	"sync"
	"time"

	"k8s.io/apimachinery/pkg/types"
	"sigs.k8s.io/controller-runtime/pkg/client"

	runtimev1alpha1 "go.wasmcloud.dev/runtime-operator/v2/api/runtime/v1alpha1"
)

const retiredHostRetention = time.Hour

// RetiredHosts tracks removed Hosts and Pods until a later insert prunes them.
type RetiredHosts struct {
	mu   sync.RWMutex
	ids  map[string]retiredHost
	pods map[string]retiredPod
}

// retiredHost tells the host process behind a retired ID from a replacement
// under the same ID.
type retiredHost struct {
	retiredAt time.Time
	// hostname is the retired host's own, its Pod IP, which a host keeps for life.
	hostname    string
	condemnedAt time.Time
	// object is the retired Host until a read finds it gone.
	object types.NamespacedName
	uid    types.UID
}

type retiredPod struct {
	condemnedAt time.Time
	retiredAt   time.Time
}

// retire records host, deleted for its Pod condemned at condemnedAt.
func (r *RetiredHosts) retire(host *runtimev1alpha1.Host, condemnedAt time.Time) {
	if r == nil || host.HostID == "" {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.ids == nil {
		r.ids = make(map[string]retiredHost)
	}
	now := time.Now()
	r.prune(now)
	if condemnedAt.IsZero() {
		condemnedAt = now
	}
	r.ids[host.HostID] = retiredHost{
		retiredAt:   now,
		hostname:    host.Hostname,
		condemnedAt: condemnedAt,
		object:      client.ObjectKeyFromObject(host),
		uid:         host.UID,
	}
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
	for id, host := range r.ids {
		if now.Sub(host.retiredAt) >= retiredHostRetention {
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

// retiredID reports whether a heartbeat under a retired ID is the retired host: from its
// Pod IP or no IP, and not started after the Pod was condemned.
func (r *RetiredHosts) retiredID(hostID, hostname string, startedAt time.Time) bool {
	if r == nil {
		return false
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	host, ok := r.ids[hostID]
	if !ok {
		return false
	}
	if host.hostname != "" && hostname != "" && hostname != host.hostname {
		return false
	}
	// A missing start time cannot distinguish a replacement from the old host.
	return startedAt.IsZero() || !startedAt.After(host.condemnedAt)
}

// awaitedHost returns the Host retired under hostID until hostGone records it gone.
func (r *RetiredHosts) awaitedHost(hostID string) (types.NamespacedName, types.UID, bool) {
	if r == nil {
		return types.NamespacedName{}, "", false
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	host, ok := r.ids[hostID]
	if !ok || host.object.Name == "" {
		return types.NamespacedName{}, "", false
	}
	return host.object, host.uid, true
}

// hostGone records that the Host retired under hostID with uid is gone.
func (r *RetiredHosts) hostGone(hostID string, uid types.UID) {
	if r == nil {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if host, ok := r.ids[hostID]; ok && host.uid == uid {
		host.object = types.NamespacedName{}
		r.ids[hostID] = host
	}
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
