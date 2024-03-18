use anyhow::{Context as _, Result};
use bytes::Bytes;
use futures::stream::empty;

use wrpc_transport_derive::{Encode, Receive, Subscribe};

#[macro_use]
mod common;

#[tokio::test]
async fn roundtrip_enum_simple() -> Result<()> {
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Receive, Subscribe)]
    enum TestEnum {
        A,
        B,
        C,
    }

    test_roundtrip_value!(TestEnum, TestEnum::A);
    test_roundtrip_value!(TestEnum, TestEnum::B);
    test_roundtrip_value!(TestEnum, TestEnum::C);

    Ok(())
}

#[tokio::test]
async fn roundtrip_enum_unnamed_variant_args() -> Result<()> {
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Receive, Subscribe)]
    enum TestEnum {
        A,
        B(String, String),
        C,
    }

    test_roundtrip_value!(TestEnum, TestEnum::A);
    test_roundtrip_value!(TestEnum, TestEnum::B("test".into(), "test2".into()));
    test_roundtrip_value!(TestEnum, TestEnum::C);

    Ok(())
}

#[tokio::test]
async fn roundtrip_enum_named_variant_args() -> Result<()> {
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Receive, Subscribe)]
    enum TestEnum {
        A,
        B { first: String, second: String },
        C,
    }

    test_roundtrip_value!(TestEnum, TestEnum::A);
    test_roundtrip_value!(
        TestEnum,
        TestEnum::B {
            first: "test".into(),
            second: "test2".into()
        }
    );
    test_roundtrip_value!(TestEnum, TestEnum::C);

    Ok(())
}

#[tokio::test]
async fn roundtrip_enum_mixed_variant_args() -> Result<()> {
    #[derive(Debug, Clone, PartialEq, Eq, Encode, Receive, Subscribe)]
    enum TestEnum {
        A,
        B { first: String, second: String },
        C(String, String),
    }

    test_roundtrip_value!(TestEnum, TestEnum::A);
    test_roundtrip_value!(
        TestEnum,
        TestEnum::B {
            first: "test".into(),
            second: "test2".into()
        }
    );
    test_roundtrip_value!(TestEnum, TestEnum::C("test3".into(), "test4".into()));

    Ok(())
}
