/// Utility macro for roundtripping a single value of a single type T
///
/// # Examples
///
/// ```rust,ignore
/// async fn roundtrip_struct_simple() -> Result<()> {
///     #[derive(Debug, Clone, PartialEq, Eq, EncodeSync, Receive, Default)]
///     struct TestStruct {
///         byte: u8,
///         string: String,
///     }
///
///     test_roundtrip_value!(
///         TestStruct,
///         TestStruct {
///             byte: 1,
///             string: "test".into(),
///         }
///     );
///
///     Ok(())
/// }
///
///```
macro_rules! test_roundtrip_value {
    ($t:ty, $struct_inst:expr) => {
        // Encode the TestStruct
        let mut buffer: Vec<u8> = Vec::new();
        let val = $struct_inst;
        val.clone()
            .encode(&mut buffer)
            .await
            .context("failed to perform encode")?;

        // Rebuild from received value
        let (received, leftover): ($t, _) =
            Receive::receive_sync(Bytes::from(buffer), &mut empty())
                .await
                .context("failed to receive")?;

        assert_eq!(received, val, "received matches original");
        assert_eq!(leftover.remaining(), 0, "payload was completely consumed");
    };
}
