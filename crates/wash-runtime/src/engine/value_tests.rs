use super::*;
use crate::engine::ctx::Ctx;

#[test]
fn linked_values_preserve_nested_payloads_and_scalar_boundaries() -> wasmtime::Result<()> {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let engine = wasmtime::Engine::default();
    let ctx = Ctx::builder("test-workload", "test-component").build();
    let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
    let scalars = vec![
        Val::Bool(false),
        Val::S8(i8::MIN),
        Val::U8(u8::MAX),
        Val::S16(i16::MIN),
        Val::U16(u16::MAX),
        Val::S32(i32::MIN),
        Val::U32(u32::MAX),
        Val::S64(i64::MIN),
        Val::U64(u64::MAX),
        Val::Float32(-1.5),
        Val::Float64(2.5),
        Val::Char('🦀'),
        Val::String("héllo".into()),
        Val::Enum("ready".into()),
        Val::Flags(vec!["read".into(), "write".into()]),
    ];
    let nested = Val::Record(vec![
        ("scalars".into(), Val::Tuple(scalars.clone())),
        ("list".into(), Val::List(vec![Val::U8(1), Val::U8(2)])),
        ("fixed".into(), Val::FixedLengthList(vec![Val::U8(3)])),
        (
            "case".into(),
            Val::Variant("ready".into(), Some(Box::new(Val::U32(4)))),
        ),
        ("empty-case".into(), Val::Variant("empty".into(), None)),
        (
            "some".into(),
            Val::Option(Some(Box::new(Val::String("value".into())))),
        ),
        ("none".into(), Val::Option(None)),
        (
            "ok".into(),
            Val::Result(Ok(Some(Box::new(Val::Bool(true))))),
        ),
        ("unit-ok".into(), Val::Result(Ok(None))),
        (
            "err".into(),
            Val::Result(Err(Some(Box::new(Val::String("failed".into()))))),
        ),
        ("unit-err".into(), Val::Result(Err(None))),
    ]);
    let mut cx = store.as_context_mut();
    assert_eq!(lower(&mut cx, &nested)?, nested);
    assert_eq!(lift(&mut cx, nested.clone())?, nested);
    let mut results = vec![Val::Bool(true); scalars.len()];
    lift_results(&mut cx, scalars.clone(), &mut results)?;
    assert_eq!(results, scalars);
    assert!(lift_results(&mut cx, vec![Val::Bool(false)], &mut []).is_err());
    assert!(lower_params(&mut cx, &[Val::U32(1)], &[]).is_err());
    let params = lower_params(&mut cx, &[Val::U32(1)], &[Type::U32])?;
    assert_eq!(params.vals, vec![Val::U32(1)]);
    params.release_identity_borrows(&mut store)?;
    Ok(())
}
