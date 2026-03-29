package main

import (
	"context"
	"errors"
	"net"
	"net/http"
	"strconv"
	"strings"
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
	RegisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string, path string) error
	DeregisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string, path string) error
}

var _ HostResolver = (*HostTracker)(nil)
var _ HostRegistry = (*HostTracker)(nil)
var _ WorkloadRegistry = (*HostTracker)(nil)

// RouteKey identifies a route by hostname and optional path prefix.
// An empty Path acts as a catch-all for the hostname.
type RouteKey struct {
	Hostname string
	Path     string
}

type HostTracker struct {
	// where to send requests that have no registered workloads
	Fallback Fallback

	lock sync.RWMutex
	// HostID to "hostname:port"
	hosts map[string]string
	// RouteKey to WorkloadIDs
	hostnames map[RouteKey]sets.Set[string]
	// WorkloadID to HostID
	workloads map[string]string
}

func (ht *HostTracker) SetupWithManager(ctx context.Context, manager ctrl.Manager) error {
	return manager.Add(ht)
}

func (ht *HostTracker) Start(ctx context.Context) error {
	ht.hosts = make(map[string]string)
	ht.hostnames = make(map[RouteKey]sets.Set[string])
	ht.workloads = make(map[string]string)
	<-ctx.Done()
	return nil
}

// resolve finds the best matching workload set for the given request.
// It first tries the longest registered path prefix, then falls back to
// the catch-all route (empty path) for the hostname.
func (ht *HostTracker) resolve(hostname, urlPath string) (sets.Set[string], bool) {
	var bestKey RouteKey
	var bestWorkloads sets.Set[string]
	found := false

	for key, workloads := range ht.hostnames {
		if key.Hostname != hostname || key.Path == "" {
			continue
		}
		if rest, ok := strings.CutPrefix(urlPath, key.Path); ok {
			atBoundary := rest == "" || rest[0] == '/' || rest[0] == '?'
			if atBoundary && len(key.Path) > len(bestKey.Path) {
				bestKey = key
				bestWorkloads = workloads
				found = true
			}
		}
	}

	if found {
		return bestWorkloads, true
	}

	// Fall back to catch-all route for this hostname
	workloads, ok := ht.hostnames[RouteKey{Hostname: hostname}]
	return workloads, ok
}

// hostnameKnown reports whether any route (path-specific or catch-all) is
// registered for the given hostname.
func (ht *HostTracker) hostnameKnown(hostname string) bool {
	for key := range ht.hostnames {
		if key.Hostname == hostname {
			return true
		}
	}
	return false
}

func (ht *HostTracker) Resolve(ctx context.Context, req *http.Request) LookupResult {
	ht.lock.RLock()
	defer ht.lock.RUnlock()

	hostname := req.Host

	workloads, ok := ht.resolve(hostname, req.URL.Path)
	if !ok {
		if !ht.hostnameKnown(hostname) {
			scheme, endpoint := ht.Fallback.InvalidHostname(hostname)
			return LookupResult{Hostname: endpoint, Scheme: scheme}
		}
		scheme, endpoint := ht.Fallback.NoWorkloads(hostname)
		return LookupResult{Hostname: endpoint, Scheme: scheme}
	}

	if workloads.Len() == 0 {
		scheme, endpoint := ht.Fallback.NoWorkloads(hostname)
		return LookupResult{Hostname: endpoint, Scheme: scheme}
	}

	// pick a random workload
	workloadID := workloads.UnsortedList()[0]

	// find the host for the workload
	// (should always exist if the workload exists)
	hostID, ok := ht.workloads[workloadID]
	if !ok {
		scheme, endpoint := ht.Fallback.NoWorkloads(hostname)
		return LookupResult{Hostname: endpoint, Scheme: scheme}
	}

	// find the hostname:port for the host
	// (should always exist if the host is healthy)
	hostAddr, ok := ht.hosts[hostID]
	if !ok {
		scheme, endpoint := ht.Fallback.NoWorkloads(hostname)
		return LookupResult{Hostname: endpoint, Scheme: scheme}
	}

	return LookupResult{
		Hostname:   hostAddr,
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

	delete(ht.hosts, hostID)
	return nil
}

func (ht *HostTracker) RegisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string, path string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	if path != "" && !strings.HasPrefix(path, "/") {
		path = "/" + path
	}
	key := RouteKey{Hostname: hostname, Path: path}
	ht.workloads[workloadID] = hostID
	if workloadSet, ok := ht.hostnames[key]; !ok {
		ht.hostnames[key] = sets.New(workloadID)
	} else {
		workloadSet.Insert(workloadID)
	}
	return nil
}

func (ht *HostTracker) DeregisterWorkload(ctx context.Context, hostID string, workloadID string, hostname string, path string) error {
	ht.lock.Lock()
	defer ht.lock.Unlock()

	if path != "" && !strings.HasPrefix(path, "/") {
		path = "/" + path
	}
	key := RouteKey{Hostname: hostname, Path: path}
	delete(ht.workloads, workloadID)
	if workloadSet, ok := ht.hostnames[key]; ok {
		workloadSet.Delete(workloadID)
		if workloadSet.Len() == 0 {
			delete(ht.hostnames, key)
		}
	}
	return nil
}
