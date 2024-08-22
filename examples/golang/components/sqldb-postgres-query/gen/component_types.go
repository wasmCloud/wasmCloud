package component

// inspired from https://github.com/moznion/go-optional

type optionKind int

const (
none optionKind = iota
some
)

type Option[T any] struct {
  kind optionKind
  val  T
}

// IsNone returns true if the option is None.
func (o Option[T]) IsNone() bool {
  return o.kind == none
}

// IsSome returns true if the option is Some.
func (o Option[T]) IsSome() bool {
  return o.kind == some
}

// Unwrap returns the value if the option is Some.
func (o Option[T]) Unwrap() T {
  if o.kind != some {
    panic("Option is None")
  }
  return o.val
}

// Set sets the value and returns it.
func (o *Option[T]) Set(val T) T {
  o.kind = some
  o.val = val
  return val
}

// Unset sets the value to None.
func (o *Option[T]) Unset() {
  o.kind = none
}

// Some is a constructor for Option[T] which represents Some.
func Some[T any](v T) Option[T] {
  return Option[T]{
    kind: some,
    val:  v,
  }
}

// None is a constructor for Option[T] which represents None.
func None[T any]() Option[T] {
  return Option[T]{
    kind: none,
  }
}

type ResultKind int

const (
resultOk ResultKind = iota
resultErr
)

type Result[T any, E any] struct {
  kind ResultKind
  resultOk   T
  resultErr  E
}

// IsOk returns true if the result is Ok.
func (r Result[T, E]) IsOk() bool {
  return r.kind == resultOk
}

// IsErr returns true if the result is Err.
func (r Result[T, E]) IsErr() bool {
  return r.kind == resultErr
}

// Unwrap returns the value if the result is Ok.
func (r Result[T, E]) Unwrap() T {
  if r.kind != resultOk {
    panic("Result is Err")
  }
  return r.resultOk
}

// UnwrapErr returns the value if the result is Err.
func (r Result[T, E]) UnwrapErr() E {
  if r.kind != resultErr {
    panic("Result is Ok")
  }
  return r.resultErr
}

// Set sets the value and returns it.
func (r *Result[T, E]) Set(val T) T {
  r.kind = resultOk
  r.resultOk = val
  return val
}

// SetErr sets the value and returns it.
func (r *Result[T, E]) SetErr(val E) E {
  r.kind = resultErr
  r.resultErr = val
  return val
}

// Ok is a constructor for Result[T, E] which represents Ok.
func Ok[T any, E any](v T) Result[T, E] {
  return Result[T, E]{
    kind: resultOk,
    resultOk:   v,
  }
}

// Err is a constructor for Result[T, E] which represents Err.
func Err[T any, E any](v E) Result[T, E] {
  return Result[T, E]{
    kind: resultErr,
    resultErr:  v,
  }
}

