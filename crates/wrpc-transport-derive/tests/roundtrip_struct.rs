use anyhow::{Context as _, Result};
use bytes::Bytes;
use futures::stream::empty;

use wrpc_transport_derive::{Encode, Receive, Subscribe};

#[macro_use]
mod common;

/// Ensure that an Encode call works on one for which it is derived
#[tokio::test]
async fn roundtrip_single_member_struct() -> Result<()> {
    #[derive(Debug, PartialEq, Eq, Encode, Receive, Default, Subscribe)]
    struct TestStruct {
        one: u32,
    }

    let mut buffer: Vec<u8> = Vec::new();
    // Encode the TestStruct
    TestStruct { one: 1 }
        .encode(&mut buffer)
        .await
        .context("failed to perform encode")?;

    // Attempt to receive the value
    let (received, leftover): (TestStruct, _) =
        Receive::receive_sync(Bytes::from(buffer), &mut empty())
            .await
            .context("failed to receive")?;

    assert_eq!(received, TestStruct { one: 1 }, "received matches");
    assert_eq!(leftover.remaining(), 0, "payload was completely consumed");

    Ok(())
}

#[tokio::test]
async fn roundtrip_struct_simple() -> Result<()> {
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Receive, Default, Subscribe)]
    struct TestStruct {
        byte: u8,
        string: String,
    }

    test_roundtrip_value!(
        TestStruct,
        TestStruct {
            byte: 1,
            string: "test".into(),
        }
    );

    Ok(())
}

#[tokio::test]
async fn roundtrip_struct_with_option() -> Result<()> {
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Receive, Default, Subscribe)]
    struct TestStruct {
        byte: u8,
        string: String,
        maybe_string: Option<String>,
    }

    test_roundtrip_value!(
        TestStruct,
        TestStruct {
            byte: 1,
            string: "test".into(),
            maybe_string: None,
        }
    );

    test_roundtrip_value!(
        TestStruct,
        TestStruct {
            byte: 1,
            string: "test".into(),
            maybe_string: Some("value".into()),
        }
    );

    Ok(())
}

#[tokio::test]
async fn roundtrip_struct_with_vec() -> Result<()> {
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Receive, Default, Subscribe)]
    struct TestStruct {
        byte: u8,
        string: String,
        strings: Vec<String>,
    }

    test_roundtrip_value!(
        TestStruct,
        TestStruct {
            byte: 1,
            string: "test".into(),
            strings: Vec::new(),
        }
    );

    test_roundtrip_value!(
        TestStruct,
        TestStruct {
            byte: 1,
            string: "test".into(),
            strings: Vec::from(["test".into(), "test".into()]),
        }
    );

    Ok(())
}
