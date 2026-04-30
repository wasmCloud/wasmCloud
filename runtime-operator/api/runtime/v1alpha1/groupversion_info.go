// Package v1alpha1 contains API Schema definitions for the runtime v1alpha1 API group.
// +kubebuilder:object:generate=true
// +groupName=runtime.wasmcloud.dev
package v1alpha1

import (
	"reflect"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/schema"
)

var (
	// GroupVersion is group version used to register these objects.
	GroupVersion = schema.GroupVersion{Group: "runtime.wasmcloud.dev", Version: "v1alpha1"}

	// SchemeBuilder is used to add go types to the GroupVersionKind scheme.
	SchemeBuilder = &Builder{GroupVersion: GroupVersion}

	// AddToScheme adds the types in this group-version to the given scheme.
	AddToScheme = SchemeBuilder.AddToScheme
)

// Builder is a local replacement for sigs.k8s.io/controller-runtime/pkg/scheme.Builder,
// kept here so this api package depends only on k8s.io/apimachinery.
// +kubebuilder:object:generate=false
type Builder struct {
	GroupVersion schema.GroupVersion
	runtime.SchemeBuilder
}

// Register adds objects to the GroupVersionKind scheme.
func (b *Builder) Register(objects ...runtime.Object) *Builder {
	b.SchemeBuilder.Register(func(scheme *runtime.Scheme) error {
		for _, obj := range objects {
			gvk := b.GroupVersion.WithKind(reflect.TypeOf(obj).Elem().Name())
			scheme.AddKnownTypeWithName(gvk, obj)
		}
		metav1.AddToGroupVersion(scheme, b.GroupVersion)
		return nil
	})
	return b
}

// AddToScheme adds the registered types to the given scheme.
func (b *Builder) AddToScheme(s *runtime.Scheme) error {
	return b.SchemeBuilder.AddToScheme(s)
}
