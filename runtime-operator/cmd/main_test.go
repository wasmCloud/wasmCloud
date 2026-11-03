package main

import (
	"reflect"
	"testing"
)

func TestNamespaceAndListFlags(t *testing.T) {
	const (
		firstNamespace  = "team-a"
		secondNamespace = "team-b"
	)
	for _, tt := range []struct {
		name, raw  string
		list       []string
		namespaces []string
	}{
		{name: "unset"},
		{name: "empty entries", raw: " , \t,\n"},
		{
			name: "single namespace", raw: " team-a ",
			list: []string{firstNamespace}, namespaces: []string{firstNamespace},
		},
		{
			name: "trim and deduplicate", raw: " team-a,,team-b,team-a, ",
			list:       []string{firstNamespace, secondNamespace, firstNamespace},
			namespaces: []string{firstNamespace, secondNamespace},
		},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := splitCSVList(tt.raw); !reflect.DeepEqual(got, tt.list) {
				t.Fatalf("splitCSVList(%q) = %v, want %v", tt.raw, got, tt.list)
			}
			got := parseNamespaceSet(tt.raw)
			if len(got) != len(tt.namespaces) || (got == nil) != (tt.namespaces == nil) {
				t.Fatalf("parseNamespaceSet(%q) = %v, want %v", tt.raw, got, tt.namespaces)
			}
			for _, namespace := range tt.namespaces {
				if _, ok := got[namespace]; !ok {
					t.Errorf("namespace %q missing from %v", namespace, got)
				}
			}
		})
	}
}
