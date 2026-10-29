//! Detect whether a store may need a Wasm GC heap before reserving pool slots.
//! Wasmtime's component resource counts omit GC heaps. Inspect the source once
//! at workload initialization; opaque or unrecognized input is conservative.

use wasmparser::{
    AbstractHeapType, CompositeInnerType, HeapType, Operator, Parser, Payload, TypeRef, ValType,
};

pub(super) fn may_use_gc(bytes: &[u8]) -> bool {
    inspect(bytes).unwrap_or(true)
}

/// Whether a reference to `ty` can point into a GC heap.
///
/// Function references never do, abstract (`func`, `nofunc`) or concrete:
/// typed function references (`(ref $f)`, `call_ref`) allocate nothing.
/// A concrete index is always a function type by the time this sees one,
/// because [`inspect`] stops at the first struct, array or continuation type
/// in a module's type section, and the type section precedes every section
/// that can name a type. So a concrete heap type that reaches here belongs
/// to a module whose types are all functions.
fn gc_heap(ty: HeapType) -> bool {
    !matches!(
        ty,
        HeapType::Abstract {
            ty: AbstractHeapType::Func | AbstractHeapType::NoFunc,
            ..
        } | HeapType::Concrete(_)
    )
}

fn gc_value(ty: ValType) -> bool {
    ty.as_reference_type()
        .is_some_and(|ty| gc_heap(ty.heap_type()))
}

/// Whether an operator from `$proposal` may allocate in, or reference, a GC
/// heap. Every operator of these proposals counts; the few whose answer
/// depends on an operand type are decided in `gc_operator` first.
macro_rules! gc_proposal {
    ($proposal:ident) => {
        matches!(
            stringify!($proposal),
            "gc" | "shared_everything_threads"
                | "exceptions"
                | "legacy_exceptions"
                | "custom_descriptors"
                | "stack_switching"
        )
    };
}

macro_rules! gc_operators {
    ($( @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
        fn gc_operator(op: Operator<'_>) -> bool {
            match &op {
                Operator::RefNull { hty } => return gc_heap(*hty),
                Operator::TypedSelect { ty } => return gc_value(*ty),
                Operator::TypedSelectMulti { tys } => return tys.iter().copied().any(gc_value),
                Operator::Block { blockty } | Operator::Loop { blockty } | Operator::If { blockty } => {
                    if let wasmparser::BlockType::Type(ty) = blockty {
                        return gc_value(*ty);
                    }
                }
                _ => {}
            }
            match op {
                $(Operator::$op $({ $($arg: _),* })? => gc_proposal!($proposal),)*
                _ => true,
            }
        }
    }
}
wasmparser::for_each_operator!(gc_operators);

fn gc_expression(mut reader: wasmparser::OperatorsReader<'_>) -> wasmparser::Result<bool> {
    while !reader.eof() {
        if gc_operator(reader.read()?) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// [`gc_expression`] for a constant expression: an initializer or offset.
fn gc_const_expr(expr: &wasmparser::ConstExpr<'_>) -> wasmparser::Result<bool> {
    gc_expression(expr.get_operators_reader())
}

fn inspect(bytes: &[u8]) -> wasmparser::Result<bool> {
    // parse_all visits nested core modules as well as the outer component.
    for payload in Parser::new(0).parse_all(bytes) {
        match payload? {
            Payload::TypeSection(types) => {
                for group in types {
                    for ty in group?.types() {
                        let CompositeInnerType::Func(func) = &ty.composite_type.inner else {
                            return Ok(true);
                        };
                        if func
                            .params()
                            .iter()
                            .chain(func.results())
                            .copied()
                            .any(gc_value)
                        {
                            return Ok(true);
                        }
                    }
                }
            }
            Payload::ImportSection(imports) => {
                for import in imports.into_imports() {
                    match import?.ty {
                        TypeRef::Tag(_) => return Ok(true),
                        TypeRef::Global(ty) if gc_value(ty.content_type) => return Ok(true),
                        TypeRef::Table(ty) if gc_heap(ty.element_type.heap_type()) => {
                            return Ok(true);
                        }
                        _ => {}
                    }
                }
            }
            Payload::TagSection(tags) if tags.count() > 0 => return Ok(true),
            Payload::TableSection(tables) => {
                for table in tables {
                    let table = table?;
                    if gc_heap(table.ty.element_type.heap_type()) {
                        return Ok(true);
                    }
                    if let wasmparser::TableInit::Expr(expr) = table.init
                        && gc_const_expr(&expr)?
                    {
                        return Ok(true);
                    }
                }
            }
            Payload::GlobalSection(globals) => {
                for global in globals {
                    let global = global?;
                    if gc_value(global.ty.content_type) || gc_const_expr(&global.init_expr)? {
                        return Ok(true);
                    }
                }
            }
            Payload::ElementSection(elements) => {
                for element in elements {
                    let element = element?;
                    if let wasmparser::ElementKind::Active { offset_expr, .. } = element.kind
                        && gc_const_expr(&offset_expr)?
                    {
                        return Ok(true);
                    }
                    if let wasmparser::ElementItems::Expressions(ty, expressions) = element.items {
                        if gc_heap(ty.heap_type()) {
                            return Ok(true);
                        }
                        for expr in expressions {
                            if gc_const_expr(&expr?)? {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
            Payload::DataSection(data) => {
                for data in data {
                    if let wasmparser::DataKind::Active { offset_expr, .. } = data?.kind
                        && gc_const_expr(&offset_expr)?
                    {
                        return Ok(true);
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                for local in body.get_locals_reader()? {
                    if gc_value(local?.1) {
                        return Ok(true);
                    }
                }
                if gc_expression(body.get_operators_reader()?)? {
                    return Ok(true);
                }
            }
            Payload::UnknownSection { .. } => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::may_use_gc;

    #[test]
    fn detects_gc_in_nested_modules_and_keeps_numeric_guests_uncharged() {
        for (module, expected) in [
            (
                "(memory 1) (table 1 funcref) (func (param i32) (result i32) local.get 0)",
                false,
            ),
            ("(type (struct (field i32)))", true),
            ("(type (array i32))", true),
            ("(table 1 externref)", true),
            ("(func (param externref))", true),
            ("(global externref (ref.null extern))", true),
            ("(func (local externref))", true),
            ("(func i32.const 1 ref.i31 drop)", true),
            ("(func ref.null any drop)", true),
            ("(tag (param i32)) (func i32.const 1 throw 0)", true),
            ("(import \"host\" \"tag\" (tag (param i32)))", true),
            ("(elem declare anyref (ref.null any))", true),
            (
                "(func unreachable block (result anyref) unreachable end drop)",
                true,
            ),
            // Typed function references are not GC: they allocate nothing.
            (
                "(type $f (func)) (table 1 (ref null $f)) \
                 (func (param (ref null $f)) (local (ref null $f)) ref.null $f drop)",
                false,
            ),
            // A struct type anywhere in the module still marks it as GC.
            (
                "(type $f (func)) (type (struct)) (func (param (ref null $f)))",
                true,
            ),
        ] {
            let bytes = wat::parse_str(format!("(component (core module {module}))")).unwrap();
            assert_eq!(may_use_gc(&bytes), expected, "{module}");
        }
        assert!(may_use_gc(b"opaque or malformed input"));
    }
}
