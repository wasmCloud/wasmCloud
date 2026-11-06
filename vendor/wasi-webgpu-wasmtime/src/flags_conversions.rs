use crate::wasi::webgpu::webgpu;

/// Generates bidirectional `TryFrom` (returning `wasmtime::Result`) + round-trip
/// tests between two bitflags types. Rows are `A_entry, B_entry;`:
/// - `Some(a), Some(b)` — map both ways.
/// - `Some(a), None` / `None, Some(b)` — no counterpart; that direction `Err`s.
///
/// Don't list composite aliases whose bits are already mapped (e.g.
/// `ColorWrites::COLOR`). For a redundant alias bit on the B side (e.g.
/// `GpuColorWrite::ALL`), add `ignore_b: <bits>,` to exempt it from the test.
macro_rules! flag_map {
    // No alias mask -> default to empty (fully exact comparison).
    (
        $amod:ident :: $a:ident => $bmod:ident :: $b:ident,
        $( $av:expr, $bv:expr );* $(;)?
    ) => {
        flag_map! {
            @build $amod :: $a => $bmod :: $b,
            ignore_b: <$bmod::$b>::empty(),
            $( $av, $bv );*
        }
    };

    // Optional alias mask: B-side bits (e.g. composite `GpuColorWrite::ALL`)
    // ignored in the A->B exact-output check; everything else stays exact.
    (
        $amod:ident :: $a:ident => $bmod:ident :: $b:ident,
        ignore_b: $ignore_b:expr,
        $( $av:expr, $bv:expr );* $(;)?
    ) => {
        flag_map! {
            @build $amod :: $a => $bmod :: $b,
            ignore_b: $ignore_b,
            $( $av, $bv );*
        }
    };

    (
        @build $amod:ident :: $a:ident => $bmod:ident :: $b:ident,
        ignore_b: $ignore_b:expr,
        $( $av:expr, $bv:expr );* $(;)?
    ) => {
        impl TryFrom<$amod::$a> for $bmod::$b {
            type Error = wasmtime::Error;
            fn try_from(a: $amod::$a) -> wasmtime::Result<$bmod::$b> {
                let mut out = <$bmod::$b>::empty();
                let mut unsupported = <$amod::$a>::empty();
                $(
                    let pair: (Option<$amod::$a>, Option<$bmod::$b>) = ($av, $bv);
                    if let (Some(av), None) = pair {
                        if a.contains(av) { unsupported |= av; }
                    }
                    if let (Some(av), Some(bv)) = pair {
                        if a.contains(av) { out |= bv; }
                    }
                )*
                if unsupported != <$amod::$a>::empty() {
                    Err(wasmtime::Error::msg(format!("unsupported flags: {unsupported:?}")))
                } else {
                    Ok(out)
                }
            }
        }

        impl TryFrom<$bmod::$b> for $amod::$a {
            type Error = wasmtime::Error;
            fn try_from(b: $bmod::$b) -> wasmtime::Result<$amod::$a> {
                let mut out = <$amod::$a>::empty();
                let mut unsupported = <$bmod::$b>::empty();
                $(
                    let pair: (Option<$amod::$a>, Option<$bmod::$b>) = ($av, $bv);
                    if let (None, Some(bv)) = pair {
                        if b.contains(bv) { unsupported |= bv; }
                    }
                    if let (Some(av), Some(bv)) = pair {
                        if b.contains(bv) { out |= av; }
                    }
                )*
                if unsupported != <$bmod::$b>::empty() {
                    Err(wasmtime::Error::msg(format!("unsupported flags: {unsupported:?}")))
                } else {
                    Ok(out)
                }
            }
        }

        const _: () = {
            // Reference the types so a typo in $a/$b fails near the macro.
            let _ = <$amod::$a>::empty;
            let _ = <$bmod::$b>::empty;
        };

        // Tests are named after the concrete flag types and direction, e.g.
        // `BufferUsages_to_GpuBufferUsage_all`, which also keeps each invocation's
        // tests uniquely named without a wrapper module.
        #[cfg(test)]
        pastey::paste! {
            // A -> B: every A flag that has a B counterpart must be consumed.
            // Flags present only in A (None on the B side) must remain and force Err.
            #[cfg(test)]
            #[allow(non_snake_case)]
            #[test]
            fn [<$a _to_ $b _all>]() {
                let a = <$amod::$a>::all();
                let mut expected_remaining = a;
                let mut expected_out = <$bmod::$b>::empty();
                $(
                    let pair: (Option<$amod::$a>, Option<$bmod::$b>) = ($av, $bv);
                    if let (Some(from), Some(to)) = pair {
                        if expected_remaining.contains(from) {
                            expected_remaining &= !from;
                            expected_out |= to;
                        }
                    }
                )*
                let result = <$bmod::$b as TryFrom<$amod::$a>>::try_from(a);
                if expected_remaining == <$amod::$a>::empty() {
                    // Ignore redundant alias bits (e.g. `GpuColorWrite::ALL`) in the
                    // exact-output check; every non-aliased bit must still match.
                    let ignore = $ignore_b;
                    let got = result.expect("expected Ok, got Err");
                    assert_eq!(got & !ignore, expected_out & !ignore);
                } else {
                    let err = result.unwrap_err();
                    assert!(
                        err.to_string().contains(&format!("{expected_remaining:?}")),
                        "error {err:?} should name unsupported flags {expected_remaining:?}"
                    );
                }
            }

            #[cfg(test)]
            #[allow(non_snake_case)]
            #[test]
            fn [<$a _to_ $b _empty>]() {
                assert_eq!(
                    <$bmod::$b as TryFrom<$amod::$a>>::try_from(<$amod::$a>::empty()).unwrap(),
                    <$bmod::$b>::empty()
                );
            }

            #[cfg(test)]
            #[allow(non_snake_case)]
            #[test]
            fn [<$b _to_ $a _all>]() {
                let b = <$bmod::$b>::all();
                let mut expected_remaining = b;
                let mut expected_out = <$amod::$a>::empty();
                $(
                    let pair: (Option<$amod::$a>, Option<$bmod::$b>) = ($av, $bv);
                    if let (Some(to), Some(from)) = pair {
                        if expected_remaining.contains(from) {
                            expected_remaining &= !from;
                            expected_out |= to;
                        }
                    }
                )*
                let result = <$amod::$a as TryFrom<$bmod::$b>>::try_from(b);
                if expected_remaining == <$bmod::$b>::empty() {
                    assert_eq!(result.unwrap(), expected_out);
                } else {
                    let err = result.unwrap_err();
                    assert!(
                        err.to_string().contains(&format!("{expected_remaining:?}")),
                        "error {err:?} should name unsupported flags {expected_remaining:?}"
                    );
                }
            }

            #[cfg(test)]
            #[allow(non_snake_case)]
            #[test]
            fn [<$b _to_ $a _empty>]() {
                assert_eq!(
                    <$amod::$a as TryFrom<$bmod::$b>>::try_from(<$bmod::$b>::empty()).unwrap(),
                    <$amod::$a>::empty()
                );
            }
        }
    };
}

flag_map! {
    wgpu_types::BufferUsages => webgpu::GpuBufferUsage,
    Some(wgpu_types::BufferUsages::MAP_READ),      Some(webgpu::GpuBufferUsage::MAP_READ);
    Some(wgpu_types::BufferUsages::MAP_WRITE),     Some(webgpu::GpuBufferUsage::MAP_WRITE);
    Some(wgpu_types::BufferUsages::COPY_SRC),      Some(webgpu::GpuBufferUsage::COPY_SRC);
    Some(wgpu_types::BufferUsages::COPY_DST),      Some(webgpu::GpuBufferUsage::COPY_DST);
    Some(wgpu_types::BufferUsages::INDEX),         Some(webgpu::GpuBufferUsage::INDEX);
    Some(wgpu_types::BufferUsages::VERTEX),        Some(webgpu::GpuBufferUsage::VERTEX);
    Some(wgpu_types::BufferUsages::UNIFORM),       Some(webgpu::GpuBufferUsage::UNIFORM);
    Some(wgpu_types::BufferUsages::STORAGE),       Some(webgpu::GpuBufferUsage::STORAGE);
    Some(wgpu_types::BufferUsages::INDIRECT),      Some(webgpu::GpuBufferUsage::INDIRECT);
    Some(wgpu_types::BufferUsages::QUERY_RESOLVE), Some(webgpu::GpuBufferUsage::QUERY_RESOLVE);
    Some(wgpu_types::BufferUsages::BLAS_INPUT),    None;
    Some(wgpu_types::BufferUsages::TLAS_INPUT),    None;
}

flag_map! {
    wgpu_types::ColorWrites => webgpu::GpuColorWrite,
    ignore_b: webgpu::GpuColorWrite::ALL,
    Some(wgpu_types::ColorWrites::RED),   Some(webgpu::GpuColorWrite::RED);
    Some(wgpu_types::ColorWrites::GREEN), Some(webgpu::GpuColorWrite::GREEN);
    Some(wgpu_types::ColorWrites::BLUE),  Some(webgpu::GpuColorWrite::BLUE);
    Some(wgpu_types::ColorWrites::ALPHA), Some(webgpu::GpuColorWrite::ALPHA);
    Some(wgpu_types::ColorWrites::ALL),   Some(webgpu::GpuColorWrite::ALL);
}

flag_map! {
    wgpu_types::ShaderStages => webgpu::GpuShaderStage,
    Some(wgpu_types::ShaderStages::NONE),            None;
    Some(wgpu_types::ShaderStages::VERTEX),          Some(webgpu::GpuShaderStage::VERTEX);
    Some(wgpu_types::ShaderStages::FRAGMENT),        Some(webgpu::GpuShaderStage::FRAGMENT);
    Some(wgpu_types::ShaderStages::COMPUTE),         Some(webgpu::GpuShaderStage::COMPUTE);
    Some(wgpu_types::ShaderStages::TASK),            None;
    Some(wgpu_types::ShaderStages::MESH),            None;
    Some(wgpu_types::ShaderStages::RAY_GENERATION),  None;
    Some(wgpu_types::ShaderStages::ANY_HIT),         None;
    Some(wgpu_types::ShaderStages::CLOSEST_HIT),     None;
    Some(wgpu_types::ShaderStages::MISS),            None;
}

flag_map! {
    wgpu_types::TextureUsages => webgpu::GpuTextureUsage,
    Some(wgpu_types::TextureUsages::COPY_SRC),          Some(webgpu::GpuTextureUsage::COPY_SRC);
    Some(wgpu_types::TextureUsages::COPY_DST),          Some(webgpu::GpuTextureUsage::COPY_DST);
    Some(wgpu_types::TextureUsages::TEXTURE_BINDING),   Some(webgpu::GpuTextureUsage::TEXTURE_BINDING);
    Some(wgpu_types::TextureUsages::STORAGE_BINDING),   Some(webgpu::GpuTextureUsage::STORAGE_BINDING);
    Some(wgpu_types::TextureUsages::RENDER_ATTACHMENT), Some(webgpu::GpuTextureUsage::RENDER_ATTACHMENT);
    None,                                               Some(webgpu::GpuTextureUsage::TRANSIENT_ATTACHMENT);
    Some(wgpu_types::TextureUsages::STORAGE_ATOMIC),    None;
    Some(wgpu_types::TextureUsages::TRANSIENT),         None;
}
