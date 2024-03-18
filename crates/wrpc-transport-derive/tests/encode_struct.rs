use anyhow::{Context as _, Result};

use wrpc_transport_derive::Encode;

/// Ensure that a basic struct with only one member (a u32) works
#[tokio::test]
async fn single_member_struct() -> Result<()> {
    #[derive(Encode, Default)]
    struct TestStruct {
        pub meaning_of_life: u32,
    }

    let mut buffer: Vec<u8> = Vec::new();
    TestStruct {
        meaning_of_life: 42,
    }
    .encode(&mut buffer)
    .await
    .context("failed to perform encode")?;
    assert_eq!(buffer.as_ref(), [42u8]);
    Ok(())
}
