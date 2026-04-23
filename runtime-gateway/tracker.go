package main

import (
	"context"
	"errors"
	"net"
	"net/http"
	"strconv"
	"sync"

	"k8s.io/apimachinery/pkg/util/sets"
	ctrl "sigs.k8s.io/controller-runtime"
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

type HostRegistry interface {
	RegisterHost(ctx context.Context, hostID string, hostname string, port int) error
	DeregisterHost(ctx context.Context, hostID string) error
}

type WorkloadRegistry interface {
	RegisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error
	DeregisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error
}

var _ HostResolver = (*HostTracker)(nil)
var _ HostRegistry = (*HostTracker)(nil)
var _ WorkloadRegistry = (*HostTracker)(nil)

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
}

func (ht *HostTracker) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	return manager.Add(ht)
}

func (ht *HostTracker) Start(ctx context.Context) error {
	ht.hosts = make(map[string]string)
	ht.hostnames = make(map[string]sets.Set[string])
	ht.workloads = make(map[string]string)
	<-ctx.Done()
	return nil
}

func (ht *HostTracker) Resolve(ctx context.Context, req *http.Request) LookupResult {
	ht.lock.RLock()
	defer ht.lock.RUnlock()

	// Per RFC 7230, the Host header may include a port (e.g.
	// "localhost:8000"). Workload hostnames are registered as bare
	// hostnames because the upstream WorkloadDeployment CRD validates
	// them as RFC 1123 names (no port allowed). Look up the exact
	// header first to preserve any existing host:port registrations,
	// then fall back to the host portion without the port. This makes
	// the gateway behave like nginx/traefik/envoy, which all match on
	// hostname regardless of the port the client appended.
	workloads, ok := ht.hostnames[req.Host]
	if !ok {
		if hostOnly, _, splitErr := net.SplitHostPort(req.Host); splitErr == nil {
			workloads, ok = ht.hostnames[hostOnly]
		}
	}
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

func (ht *HostTracker) RegisterHost(ctx context.Context, hostID string, hostname string, port int) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	ht.hosts[hostID] = net.JoinHostPort(hostname, strconv.Itoa(port))
	return nil
}

func (ht *HostTracker) DeregisterHost(ctx context.Context, hostID string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	// Collect all workload IDs that were assigned to this host so we can clean
	// them out of ht.workloads and ht.hostnames. Leaving them would leak memory
	// proportional to workload churn and would cause stale hostname mappings if
	// a new host ever reuses the same hostID.
	var affected []string
	for workloadID, hID := range ht.workloads {
		if hID == hostID {
			affected = append(affected, workloadID)
		}
	}
	for _, workloadID := range affected {
		delete(ht.workloads, workloadID)
		for hostname, workloadSet := range ht.hostnames {
			workloadSet.Delete(workloadID)
			if workloadSet.Len() == 0 {
				delete(ht.hostnames, hostname)
			}
		}
	}

	delete(ht.hosts, hostID)
	return nil
}

func (ht *HostTracker) RegisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	ht.workloads[workloadID] = hostID
	if workloadSet, ok := ht.hostnames[hostname]; !ok {
		ht.hostnames[hostname] = sets.New(workloadID)
	} else {
		workloadSet.Insert(workloadID)
	}
	return nil
}

func (ht *HostTracker) DeregisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	delete(ht.workloads, workloadID)
	if workloadSet, ok := ht.hostnames[hostname]; ok {
		workloadSet.Delete(workloadID)
		if workloadSet.Len() == 0 {
			delete(ht.hostnames, hostname)
		}
	}
	return nil
}
