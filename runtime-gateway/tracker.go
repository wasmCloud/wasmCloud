package main

import (
	"context"
	"errors"
	"math/rand/v2"
	"net"
	"net/http"
	"strconv"
	"sync"

	"k8s.io/apimachinery/pkg/types"
	"k8s.io/apimachinery/pkg/util/sets"
)

var ErrHostnameNotFound = errors.New("hostname not found")

type LookupResult struct {
	Hostname   string
	Scheme     string
	WorkloadID string
}

type HostResolver interface {
	Resolve(ctx context.Context, req *http.Request) LookupResult
}

// HostRegistry tracks the hosts requests can be forwarded to. Registrations are
// keyed by the Host object's key so that deregistering needs nothing but the
// key of an object that may already be gone from the API server.
type HostRegistry interface {
	RegisterHost(ctx context.Context, key types.NamespacedName, hostID string, hostname string, port int) error
	DeregisterHost(ctx context.Context, key types.NamespacedName) error
	// SuspendHost stops traffic to a host that is not Ready but keeps its
	// workload routes, so they serve again when the host is Ready.
	SuspendHost(ctx context.Context, key types.NamespacedName, hostID string) error
}

// WorkloadRegistry tracks which workloads serve which hostname. As with
// HostRegistry, registrations are keyed by the Workload object's key.
type WorkloadRegistry interface {
	RegisterWorkload(ctx context.Context, key types.NamespacedName, hostID string, workloadID string, hostname string) error
	DeregisterWorkload(ctx context.Context, key types.NamespacedName) error
}

var _ HostResolver = (*HostTracker)(nil)
var _ HostRegistry = (*HostTracker)(nil)
var _ WorkloadRegistry = (*HostTracker)(nil)

// workloadRoute is everything a registered Workload contributes to the routing
// tables, retained so the entry can be undone from the object's key alone.
type workloadRoute struct {
	hostID     string
	workloadID string
	hostname   string
}

type HostTracker struct {
	// where to send requests that have no registered workloads
	Fallback Fallback

	lock sync.RWMutex
	// HostID to "hostname:port"
	hosts map[string]string
	// hostname to WorkloadID
	hostnames map[string]sets.Set[string]
	// WorkloadID to HostID
	workloads map[string]string
	// Host object key to HostID
	hostKeys map[types.NamespacedName]string
	// Workload object key to the route it registered
	workloadKeys map[types.NamespacedName]workloadRoute
	// hostname to the WorkloadIDs whose host takes traffic, so a lookup picks
	// one without scanning replicas on suspended hosts
	ready map[string][]string
}

func newHostTracker(fallback Fallback) *HostTracker {
	return &HostTracker{
		Fallback:     fallback,
		hosts:        make(map[string]string),
		hostnames:    make(map[string]sets.Set[string]),
		workloads:    make(map[string]string),
		hostKeys:     make(map[types.NamespacedName]string),
		workloadKeys: make(map[types.NamespacedName]workloadRoute),
		ready:        make(map[string][]string),
	}
}

func (ht *HostTracker) Resolve(ctx context.Context, req *http.Request) LookupResult {
	ht.lock.RLock()
	defer ht.lock.RUnlock()

	if _, ok := ht.hostnames[req.Host]; !ok {
		scheme, endpoint := ht.Fallback.InvalidHostname(req.Host)
		return LookupResult{
			Hostname: endpoint,
			Scheme:   scheme,
		}
	}

	// pick a random workload whose host takes traffic
	if ready := ht.ready[req.Host]; len(ready) > 0 {
		workloadID := ready[rand.IntN(len(ready))]
		if hostname, ok := ht.hosts[ht.workloads[workloadID]]; ok {
			return LookupResult{
				Hostname:   hostname,
				Scheme:     "http",
				WorkloadID: workloadID,
			}
		}
	}

	scheme, endpoint := ht.Fallback.NoWorkloads(req.Host)
	return LookupResult{
		Hostname: endpoint,
		Scheme:   scheme,
	}
}

func (ht *HostTracker) RegisterHost(ctx context.Context, key types.NamespacedName, hostID string, hostname string, port int) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	ht.setHostKey(key, hostID)
	_, wasReady := ht.hosts[hostID]
	ht.hosts[hostID] = net.JoinHostPort(hostname, strconv.Itoa(port))
	if !wasReady {
		ht.refreshHostReady(hostID)
	}
	return nil
}

// The caller must hold ht.lock.
func (ht *HostTracker) setHostKey(key types.NamespacedName, hostID string) {
	// A Host object that comes back under a new ID — a host pod that restarted
	// and re-registered under the same object name — would otherwise leave its
	// previous ID routing traffic.
	if prev, ok := ht.hostKeys[key]; ok && prev != hostID {
		ht.removeHost(prev)
	}
	ht.hostKeys[key] = hostID
}

func (ht *HostTracker) DeregisterHost(ctx context.Context, key types.NamespacedName) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	hostID, ok := ht.hostKeys[key]
	if !ok {
		return nil
	}
	delete(ht.hostKeys, key)
	ht.removeHost(hostID)
	return nil
}

func (ht *HostTracker) SuspendHost(ctx context.Context, key types.NamespacedName, hostID string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	// Record the key even for a host never seen Ready, so deleting it drops
	// the routes its workloads registered meanwhile.
	ht.setHostKey(key, hostID)
	if _, ok := ht.hosts[hostID]; ok {
		delete(ht.hosts, hostID)
		ht.refreshHostReady(hostID)
	}
	return nil
}

// removeHost drops a host along with every workload placed on it. Leaving the
// workloads would leak memory proportional to workload churn and would cause
// stale hostname mappings if a new host ever reuses the same hostID.
//
// The caller must hold ht.lock.
func (ht *HostTracker) removeHost(hostID string) {
	for key, route := range ht.workloadKeys {
		if route.hostID == hostID {
			delete(ht.workloadKeys, key)
		}
	}
	affected := sets.New[string]()
	for workloadID, hID := range ht.workloads {
		if hID != hostID {
			continue
		}
		delete(ht.workloads, workloadID)
		for hostname, workloadSet := range ht.hostnames {
			if !workloadSet.Has(workloadID) {
				continue
			}
			affected.Insert(hostname)
			workloadSet.Delete(workloadID)
			if workloadSet.Len() == 0 {
				delete(ht.hostnames, hostname)
			}
		}
	}

	delete(ht.hosts, hostID)
	for hostname := range affected {
		ht.refreshReady(hostname)
	}
}

func (ht *HostTracker) RegisterWorkload(ctx context.Context, key types.NamespacedName, hostID string, workloadID string, hostname string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	route := workloadRoute{hostID: hostID, workloadID: workloadID, hostname: hostname}
	// A workload that moved to another host, or whose routing hostname changed,
	// must not keep serving its previous hostname.
	if prev, ok := ht.workloadKeys[key]; ok && prev != route {
		ht.removeWorkload(prev)
	}

	// Every Workload reconcile registers again; an unchanged route keeps the
	// hostname's ready workloads as they are.
	changed := ht.workloads[workloadID] != hostID || !ht.hostnames[hostname].Has(workloadID)
	ht.workloadKeys[key] = route
	ht.workloads[workloadID] = hostID
	if workloadSet, ok := ht.hostnames[hostname]; !ok {
		ht.hostnames[hostname] = sets.New(workloadID)
	} else {
		workloadSet.Insert(workloadID)
	}
	if changed {
		ht.refreshReady(hostname)
	}
	return nil
}

func (ht *HostTracker) DeregisterWorkload(ctx context.Context, key types.NamespacedName) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	route, ok := ht.workloadKeys[key]
	if !ok {
		return nil
	}
	delete(ht.workloadKeys, key)
	ht.removeWorkload(route)
	return nil
}

// The caller must hold ht.lock.
func (ht *HostTracker) removeWorkload(route workloadRoute) {
	delete(ht.workloads, route.workloadID)
	if workloadSet, ok := ht.hostnames[route.hostname]; ok {
		workloadSet.Delete(route.workloadID)
		if workloadSet.Len() == 0 {
			delete(ht.hostnames, route.hostname)
		}
	}
	ht.refreshReady(route.hostname)
}

// refreshReady rebuilds the ready workloads of one hostname.
// The caller must hold ht.lock.
func (ht *HostTracker) refreshReady(hostname string) {
	var ready []string
	for workloadID := range ht.hostnames[hostname] {
		hostID, ok := ht.workloads[workloadID]
		if !ok {
			continue
		}
		if _, ok := ht.hosts[hostID]; ok {
			ready = append(ready, workloadID)
		}
	}
	if len(ready) == 0 {
		delete(ht.ready, hostname)
		return
	}
	ht.ready[hostname] = ready
}

// refreshHostReady rebuilds the ready workloads of every hostname hostID serves.
// The caller must hold ht.lock.
func (ht *HostTracker) refreshHostReady(hostID string) {
	for hostname, workloadSet := range ht.hostnames {
		for workloadID := range workloadSet {
			if ht.workloads[workloadID] == hostID {
				ht.refreshReady(hostname)
				break
			}
		}
	}
}
