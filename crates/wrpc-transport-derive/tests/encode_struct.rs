use anyhow::{Context as _, Result};

use wrpc_transport_derive::EncodeSync;

/// Ensure that a basic struct with only one member (a u32) works
#[test]
fn single_member_struct() -> Result<()> {
    #[derive(EncodeSync, Default)]
    struct TestStruct {
        pub meaning_of_life: u32,
    }

    let mut buffer: Vec<u8> = Vec::new();
    TestStruct {
        meaning_of_life: 42,
    }
    .encode_sync(&mut buffer)
    .context("failed to perform encode")?;
    assert_eq!(buffer.as_ref(), [42u8]);
    Ok(())
}
