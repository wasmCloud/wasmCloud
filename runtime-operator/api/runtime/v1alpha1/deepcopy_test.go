package v1alpha1

import (
	"fmt"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/google/go-cmp/cmp"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
)

var (
	packagePath = reflect.TypeFor[Workload]().PkgPath()
	timeType    = reflect.TypeFor[metav1.Time]()
)

// kinds returns the types this package registers with a scheme.
func kinds(t *testing.T) []reflect.Type {
	t.Helper()
	scheme := runtime.NewScheme()
	if err := AddToScheme(scheme); err != nil {
		t.Fatalf("add to scheme: %v", err)
	}
	var kinds []reflect.Type
	for _, typ := range scheme.KnownTypes(GroupVersion) {
		if typ.PkgPath() == packagePath {
			kinds = append(kinds, typ)
		}
	}
	slices.SortFunc(kinds, func(a, b reflect.Type) int { return strings.Compare(a.Name(), b.Name()) })
	if len(kinds) == 0 {
		t.Fatal("no kinds registered")
	}
	return kinds
}

// copyableTypes returns every type of this package with a generated DeepCopy
// that a registered kind contains.
func copyableTypes(t *testing.T) []reflect.Type {
	t.Helper()
	seen := map[reflect.Type]bool{}
	var visit func(reflect.Type)
	visit = func(typ reflect.Type) {
		switch typ.Kind() {
		case reflect.Pointer, reflect.Slice, reflect.Array:
			visit(typ.Elem())
		case reflect.Map:
			visit(typ.Key())
			visit(typ.Elem())
		case reflect.Struct:
			if seen[typ] {
				return
			}
			seen[typ] = true
			for i := range typ.NumField() {
				visit(typ.Field(i).Type)
			}
		}
	}
	for _, kind := range kinds(t) {
		visit(kind)
	}
	var types []reflect.Type
	for typ := range seen {
		if _, ok := reflect.PointerTo(typ).MethodByName("DeepCopy"); ok && typ.PkgPath() == packagePath {
			types = append(types, typ)
		}
	}
	slices.SortFunc(types, func(a, b reflect.Type) int { return strings.Compare(a.Name(), b.Name()) })
	return types
}

// filler sets every field it reaches to a distinct non-zero value, allocating
// each pointer, slice and map on the way. Two fillers fill a type identically.
type filler struct {
	t *testing.T
	n int
}

func filled(t *testing.T, typ reflect.Type) reflect.Value {
	t.Helper()
	v := reflect.New(typ)
	(&filler{t: t}).fill(v.Elem())
	return v
}

func (f *filler) next() int {
	f.n++
	return f.n
}

func (f *filler) fill(v reflect.Value) {
	if v.Type() == timeType {
		v.Set(reflect.ValueOf(metav1.NewTime(time.Unix(int64(f.next()), 0).UTC())))
		return
	}
	switch v.Kind() {
	case reflect.String:
		v.SetString(fmt.Sprintf("value-%d", f.next()))
	case reflect.Bool:
		v.SetBool(true)
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		v.SetInt(int64(f.next()))
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		v.SetUint(uint64(f.next()))
	case reflect.Float32, reflect.Float64:
		v.SetFloat(float64(f.next()))
	case reflect.Pointer:
		v.Set(reflect.New(v.Type().Elem()))
		f.fill(v.Elem())
	case reflect.Slice:
		v.Set(reflect.MakeSlice(v.Type(), 2, 2))
		for i := range v.Len() {
			f.fill(v.Index(i))
		}
	case reflect.Map:
		m := reflect.MakeMap(v.Type())
		for range 2 {
			key, elem := reflect.New(v.Type().Key()).Elem(), reflect.New(v.Type().Elem()).Elem()
			f.fill(key)
			f.fill(elem)
			m.SetMapIndex(key, elem)
		}
		v.Set(m)
	case reflect.Struct:
		for i := range v.NumField() {
			if !v.Type().Field(i).IsExported() {
				f.t.Fatalf("fill: %s has unexported field %s; teach fill to set it", v.Type(), v.Type().Field(i).Name)
			}
			f.fill(v.Field(i))
		}
	default:
		f.t.Fatalf("fill: unhandled %s", v.Type())
	}
}

// scribble changes every value reachable from v in place, writing through each
// pointer, slice and map rather than replacing it, so any of them a copy
// shares with its original shows up as a change to the original.
func scribble(v reflect.Value) {
	if v.Type() == timeType {
		v.Set(reflect.ValueOf(metav1.NewTime(v.Interface().(metav1.Time).Add(time.Hour))))
		return
	}
	switch v.Kind() {
	case reflect.String:
		v.SetString(v.String() + "-changed")
	case reflect.Bool:
		v.SetBool(!v.Bool())
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		v.SetInt(v.Int() + 1)
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		v.SetUint(v.Uint() + 1)
	case reflect.Float32, reflect.Float64:
		v.SetFloat(v.Float() + 1)
	case reflect.Pointer:
		if !v.IsNil() {
			scribble(v.Elem())
		}
	case reflect.Slice:
		for i := range v.Len() {
			scribble(v.Index(i))
		}
	case reflect.Map:
		for _, key := range v.MapKeys() {
			elem := reflect.New(v.Type().Elem()).Elem()
			elem.Set(v.MapIndex(key))
			scribble(elem)
			v.SetMapIndex(key, elem)
		}
	case reflect.Struct:
		for i := range v.NumField() {
			scribble(v.Field(i))
		}
	}
}

// emptied sets every slice and map reachable through nested structs to an
// empty, non-nil value, leaving pointers nil.
func emptied(typ reflect.Type) reflect.Value {
	v := reflect.New(typ)
	var empty func(reflect.Value)
	empty = func(v reflect.Value) {
		switch v.Kind() {
		case reflect.Slice:
			v.Set(reflect.MakeSlice(v.Type(), 0, 0))
		case reflect.Map:
			v.Set(reflect.MakeMap(v.Type()))
		case reflect.Struct:
			for i := range v.NumField() {
				if v.Type().Field(i).IsExported() {
					empty(v.Field(i))
				}
			}
		}
	}
	empty(v.Elem())
	return v
}

func deepCopy(v reflect.Value) reflect.Value {
	return v.MethodByName("DeepCopy").Call(nil)[0]
}

// Controllers mutate the objects they read, and controller-runtime's cache
// hands out deep copies so a mutation never reaches the shared informer store.
// A copy that shares a map, slice or pointer with its original breaks that.
func TestDeepCopySharesNothingWithTheOriginal(t *testing.T) {
	for _, typ := range copyableTypes(t) {
		t.Run(typ.Name(), func(t *testing.T) {
			original, want := filled(t, typ), filled(t, typ)
			copied := deepCopy(original)
			// Every zero-size allocation shares one address.
			if typ.Size() > 0 && copied.Pointer() == original.Pointer() {
				t.Fatal("DeepCopy returned its receiver")
			}
			if diff := cmp.Diff(original.Interface(), copied.Interface()); diff != "" {
				t.Fatalf("copy differs from the original (-original +copy):\n%s", diff)
			}

			scribble(copied.Elem())
			if diff := cmp.Diff(want.Interface(), original.Interface()); diff != "" {
				t.Errorf("changing the copy changed the original (-want +got):\n%s", diff)
			}
			if typ.Size() > 0 && cmp.Equal(want.Interface(), copied.Interface()) {
				t.Error("scribble left the copy unchanged; the test proves nothing")
			}
		})
	}
}

func TestDeepCopyObjectSharesNothingWithTheOriginal(t *testing.T) {
	for _, typ := range kinds(t) {
		t.Run(typ.Name(), func(t *testing.T) {
			original, want := filled(t, typ), filled(t, typ)
			copied := original.Interface().(runtime.Object).DeepCopyObject()
			if diff := cmp.Diff(original.Interface(), copied); diff != "" {
				t.Fatalf("copy differs from the original (-original +copy):\n%s", diff)
			}

			scribble(reflect.ValueOf(copied).Elem())
			if diff := cmp.Diff(want.Interface(), original.Interface()); diff != "" {
				t.Errorf("changing the copy changed the original (-want +got):\n%s", diff)
			}
		})
	}
}

// Nil and empty differ for serialization and for apply, so a copy keeps
// whichever its original had.
func TestDeepCopyKeepsNilAndEmptyApart(t *testing.T) {
	for _, typ := range copyableTypes(t) {
		t.Run(typ.Name(), func(t *testing.T) {
			none := reflect.Zero(reflect.PointerTo(typ))
			if copied := deepCopy(none); !copied.IsNil() {
				t.Errorf("DeepCopy of a nil %s = %v, want nil", typ.Name(), copied)
			}
			if obj, ok := none.Interface().(runtime.Object); ok && obj.DeepCopyObject() != nil {
				t.Errorf("DeepCopyObject of a nil %s is not nil", typ.Name())
			}

			for name, original := range map[string]reflect.Value{
				"zero":    reflect.New(typ),
				"emptied": emptied(typ),
			} {
				copied := deepCopy(original)
				if !reflect.DeepEqual(original.Interface(), copied.Interface()) {
					t.Errorf("%s %s copied as %+v, want %+v", name, typ.Name(), copied.Elem(), original.Elem())
				}
			}
		})
	}
}
