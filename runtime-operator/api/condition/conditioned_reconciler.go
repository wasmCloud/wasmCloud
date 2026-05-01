package condition

import (
	"context"
	"errors"
	"fmt"
	"time"

	"k8s.io/apimachinery/pkg/runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/reconcile"
)

var errStatusUnknown = errors.New("status unknown")
var errNoop = errors.New("no-op")
var errStatusSkip = errors.New("skipping reconciliation")

// ErrSkipReconciliation is an error that indicates the reconciliation should be skipped.
// Used to indicate that the object is not ready for reconciliation or not owned by the controller.
// Only checked in PreHooks.
func ErrSkipReconciliation() error {
	return errStatusSkip
}

// ErrStatusUnknown wraps an error with a status unknown error.
// Used to indicate the status of a given condition is unknown during reconciliation.
func ErrStatusUnknown(e error) error {
	return fmt.Errorf("%w: %v", errStatusUnknown, e)
}

// IsStatusUnknown reports whether err was produced by ErrStatusUnknown.
// Exposes the unexported sentinel for callers that need to
// distinguish "we're waiting on something" from a hard error.
func IsStatusUnknown(err error) bool {
	return errors.Is(err, errStatusUnknown)
}

// ErrNoop is an error that indicates no changes should be made to the status.
// Used to indicate the reconciler is not ready to change the condition status yet.
func ErrNoop() error {
	return errNoop
}

// ConditionedType is an interface for objects that have a ConditionedStatus.
// See `condition.ConditionedStatus` for more information.
// +kubebuilder:object:generate=false
type ConditionedType interface {
	client.Object
	ConditionedStatus() *ConditionedStatus
	InitializeConditionedStatus()
}

// AnyConditionedReconciler is an interface for reconcilers that reconcile objects with conditions.
// The ConditionedReconciler is a typed version of this interface.
// +kubebuilder:object:generate=false
type AnyConditionedReconciler interface {
	Reconcile(context.Context, reconcile.Request) (reconcile.Result, error)
}

// FinalizerFunc is a function that finalizes an object.
// +kubebuilder:object:generate=false
type FinalizerFunc[T ConditionedType] func(context.Context, T) error

// HandleFinalizer ensures that the finalizer is present on the object.
// If the finalizer is not present, it adds it and updates the object.
// If the finalizer is present, it calls the finalizeFunc and removes the finalizer.
// It returns true if the object was updated. Reconcilers can use this to requeue.
func HandleFinalizer[T ConditionedType](
	ctx context.Context,
	c client.Client,
	obj T,
	finalizer string,
	finalizeFunc FinalizerFunc[T]) (bool, error) {
	objCopy := obj.DeepCopyObject().(T)
	if objCopy.GetDeletionTimestamp().IsZero() && !controllerutil.ContainsFinalizer(objCopy, finalizer) {
		objCopy.InitializeConditionedStatus()
		controllerutil.AddFinalizer(objCopy, finalizer)
		err := c.Update(ctx, objCopy)
		return err == nil, err
	}

	if !objCopy.GetDeletionTimestamp().IsZero() && controllerutil.ContainsFinalizer(objCopy, finalizer) {
		if err := finalizeFunc(ctx, objCopy); err != nil {
			return false, err
		}
		controllerutil.RemoveFinalizer(objCopy, finalizer)
		return true, c.Update(ctx, objCopy)
	}

	return false, nil
}

// NewConditionedReconciler creates a new Typed ConditionedReconciler.
// Objects will be reconciled at the given interval.
func NewConditionedReconciler[T ConditionedType](
	c client.Client,
	scheme *runtime.Scheme,
	obj T,
	interval time.Duration,
) *ConditionedReconciler[T] {
	return &ConditionedReconciler[T]{
		obj:           obj,
		client:        c,
		scheme:        scheme,
		interval:      interval,
		finalizerFunc: func(context.Context, T) error { return nil },
	}
}

type conditionFunc[T ConditionedType] func(context.Context, T) error

type conditionPair[T ConditionedType] struct {
	condition ConditionType
	fn        conditionFunc[T]
}

// ConditionedReconciler is a reconciler that reconciles objects with many conditions.
// It encodes the core kubernetes pattern of reconciling objects with conditions & finalizer.
// Each condition is a function that updates the object's status.
// Functions are executed in first-in/first-out order, and all functions are called for each reconciliation.
// The status is updated after all functions have been called.
// A finalizer name is required to use the finalizer, and the finalizer function is called before the object is deleted.
// A useful pattern is to have a "Ready" condition as the last condition, validating the status of all previous ones.
// +kubebuilder:object:generate=false
type ConditionedReconciler[T ConditionedType] struct {
	obj           T
	client        client.Client
	scheme        *runtime.Scheme
	interval      time.Duration
	beforeHooks   []conditionFunc[T]
	afterHooks    []conditionFunc[T]
	conditions    []conditionPair[T]
	finalizerFunc func(context.Context, T) error
	finalizerName string
}

// AddPreHook adds a function to be called before the conditions are evaluated.
// Functions are executed in first-in/first-out order.
// A failure in a pre-hook will prevent the conditions from being evaluated.
// To completely skip reconciliation, return `ErrSkipReconciliation`.
func (r *ConditionedReconciler[T]) AddPreHook(fn func(context.Context, T) error) {
	r.beforeHooks = append(r.beforeHooks, fn)
}

// AddPostHook adds a function to be called after the conditions are evaluated.
// Functions are executed in first-in/first-out order.
// Post-hooks are AFTER conditions, and AFTER the object status has been updated.
// To skip the remaining post-hooks, return `ErrSkipReconciliation`.
func (r *ConditionedReconciler[T]) AddPostHook(fn func(context.Context, T) error) {
	r.afterHooks = append(r.afterHooks, fn)
}

// SetCondition sets a reconcile function for the given condition type.
func (r *ConditionedReconciler[T]) SetCondition(ct ConditionType, fn func(context.Context, T) error) {
	r.conditions = append(r.conditions, conditionPair[T]{condition: ct, fn: fn})
}

// SetFinalizer sets the finalizer name and function for the reconciler.
// Only one finalizer per controller.
func (r *ConditionedReconciler[T]) SetFinalizer(finalizer string, fn func(context.Context, T) error) {
	r.finalizerName = finalizer
	r.finalizerFunc = fn
}

type ctxKey string

type ReconcilerContext struct {
	ForceUpdate       bool
	ForceRequeue      bool
	ReconcileInterval time.Duration
}

const ctxKeyReconcilerContext = ctxKey("reconciler-context")

// GetReconcilerContext retrieves the ReconcilerContext from the context.
func GetReconcilerContext(ctx context.Context) *ReconcilerContext {
	if v, ok := ctx.Value(ctxKeyReconcilerContext).(*ReconcilerContext); ok {
		return v
	}
	// return a dummy
	return &ReconcilerContext{}
}

// ForceStatusUpdate forces a full "Status" update, regardless if conditions have changed.
func ForceStatusUpdate(ctx context.Context) {
	GetReconcilerContext(ctx).ForceUpdate = true
}

// ForceRequeue forces an immediate requeue, regardless if status has changed.
func ForceRequeue(ctx context.Context) {
	GetReconcilerContext(ctx).ForceRequeue = true
}

// Reconcile reconciles the object with the given request.
// implements the `AnyConditionedReconciler` and `reconcile.Reconciler` interfaces.
func (r *ConditionedReconciler[T]) Reconcile(ctx context.Context, req reconcile.Request) (reconcile.Result, error) {
	obj := r.obj.DeepCopyObject().(T)
	if err := r.client.Get(ctx, req.NamespacedName, obj); err != nil {
		return reconcile.Result{}, client.IgnoreNotFound(err)
	}

	// NOTE(lxf): this means we only auto-initialize conditions if finalizer name is set.
	if r.finalizerName != "" {
		finalizerChanged, finalizerErr := HandleFinalizer(
			ctx,
			r.client,
			obj,
			r.finalizerName, r.finalizerFunc)
		if finalizerErr != nil {
			return reconcile.Result{}, finalizerErr
		}

		if finalizerChanged {
			return reconcile.Result{Requeue: true}, nil
		}
	}

	originalObject := obj.DeepCopyObject().(T)

	reconcilerCtx := &ReconcilerContext{
		ReconcileInterval: r.interval,
	}
	condCtx := context.WithValue(ctx, ctxKeyReconcilerContext, reconcilerCtx)

	for _, hook := range r.beforeHooks {
		if err := hook(condCtx, obj); err != nil {
			if errors.Is(err, errStatusSkip) {
				// If the pre-hook returns ErrSkipReconciliation, we skip the reconciliation.
				// This is useful for cases where we need to wait for a resource to be ready.
				return reconcile.Result{RequeueAfter: reconcilerCtx.ReconcileInterval}, nil
			}

			return reconcile.Result{}, err
		}
	}

	conditions := obj.ConditionedStatus()
	for _, condEntry := range r.conditions {
		currentCondition := conditions.GetCondition(condEntry.condition)

		condErr := condEntry.fn(condCtx, obj)
		if condErr != nil {
			if errors.Is(condErr, errStatusUnknown) {
				conditions.SetConditions(UnknownCondition(condEntry.condition, "Reconcile", condErr.Error()))
			} else if errors.Is(condErr, errNoop) {
				// NOTE(lxf): if the condition function returns ErrNoop, we skip the condition.
				// This is useful for cases where we need to wait for a resource to be ready.
				continue
			} else if errors.Is(condErr, errStatusSkip) {
				// If the condition function returns ErrSkipReconciliation, we cut the reconciliation short.
				conditions.SetConditions(ReadyCondition(condEntry.condition))
				break
			} else {
				conditions.SetConditions(ErrorCondition(condEntry.condition, "Reconcile", condErr))
			}
		} else {
			conditions.SetConditions(ReadyCondition(condEntry.condition))
		}

		// NOTE(lxf): we only update the status if the condition has changed.
		if !currentCondition.Equal(conditions.GetCondition(condEntry.condition)) {
			// NOTE(lxf): we always bail out of the loop to avoid partial updates.
			// This also ensures conditions are updated in the correct order.
			reconcilerCtx.ForceRequeue = true
			reconcilerCtx.ForceUpdate = true
			break
		}
	}

	if reconcilerCtx.ForceUpdate {
		if err := r.client.Status().Patch(ctx, obj, client.MergeFrom(originalObject)); err != nil {
			return reconcile.Result{}, err
		}
	}

	for _, hook := range r.afterHooks {
		if err := hook(ctx, obj); err != nil {
			if errors.Is(err, errStatusSkip) {
				// If the post-hook returns ErrSkipReconciliation, we skip the remaining hooks.
				break
			}

			return reconcile.Result{}, err
		}
	}

	if reconcilerCtx.ForceRequeue {
		return reconcile.Result{RequeueAfter: time.Second}, nil
	}

	return reconcile.Result{RequeueAfter: reconcilerCtx.ReconcileInterval}, nil
}
