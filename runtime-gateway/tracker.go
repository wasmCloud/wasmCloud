package main

import (
	"context"
	"errors"
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
}

func newHostTracker(fallback Fallback) *HostTracker {
	return &HostTracker{
		Fallback:     fallback,
		hosts:        make(map[string]string),
		hostnames:    make(map[string]sets.Set[string]),
		workloads:    make(map[string]string),
		hostKeys:     make(map[types.NamespacedName]string),
		workloadKeys: make(map[types.NamespacedName]workloadRoute),
	}
}

func (ht *HostTracker) Resolve(ctx context.Context, req *http.Request) LookupResult {
	ht.lock.RLock()
	defer ht.lock.RUnlock()

	workloads, ok := ht.hostnames[req.Host]
	if !ok {
		scheme, endpoint := ht.Fallback.InvalidHostname(req.Host)
		return LookupResult{
			Hostname: endpoint,
			Scheme:   scheme,
		}
	}

	if workloads.Len() == 0 {
		scheme, endpoint := ht.Fallback.NoWorkloads(req.Host)
		return LookupResult{
			Hostname: endpoint,
			Scheme:   scheme,
		}
	}

	// pick a random workload
	workloadID := workloads.UnsortedList()[0]

	// find the host for the workload
	// (should always exist if the workload exists)
	hostID, ok := ht.workloads[workloadID]
	if !ok {
		scheme, endpoint := ht.Fallback.NoWorkloads(req.Host)
		return LookupResult{
			Hostname: endpoint,
			Scheme:   scheme,
		}
	}

	// find the hostname:port for the host
	// (should always exist if the host is healthy)
	hostname, ok := ht.hosts[hostID]
	if !ok {
		scheme, endpoint := ht.Fallback.NoWorkloads(req.Host)
		return LookupResult{
			Hostname: endpoint,
			Scheme:   scheme,
		}
	}

	return LookupResult{
		Hostname:   hostname,
		Scheme:     "http",
		WorkloadID: workloadID,
	}
}

func (ht *HostTracker) RegisterHost(ctx context.Context, key types.NamespacedName, hostID string, hostname string, port int) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	// A Host object that comes back under a new ID — a host pod that restarted
	// and re-registered under the same object name — would otherwise leave its
	// previous ID routing traffic.
	if prev, ok := ht.hostKeys[key]; ok && prev != hostID {
		ht.removeHost(prev)
	}

	ht.hostKeys[key] = hostID
	ht.hosts[hostID] = net.JoinHostPort(hostname, strconv.Itoa(port))
	return nil
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
	for workloadID, hID := range ht.workloads {
		if hID != hostID {
			continue
		}
		delete(ht.workloads, workloadID)
		for hostname, workloadSet := range ht.hostnames {
			workloadSet.Delete(workloadID)
			if workloadSet.Len() == 0 {
				delete(ht.hostnames, hostname)
			}
		}
	}

	delete(ht.hosts, hostID)
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

	ht.workloadKeys[key] = route
	ht.workloads[workloadID] = hostID
	if workloadSet, ok := ht.hostnames[hostname]; !ok {
		ht.hostnames[hostname] = sets.New(workloadID)
	} else {
		workloadSet.Insert(workloadID)
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
}
