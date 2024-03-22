use std::io::{Read as _, Write as _};

use wasmcloud_actor::wasi::keyvalue;
use wasmcloud_actor::{InputStreamReader, OutputStreamWriter};

pub fn run_atomic_test() {
    let bucket = keyvalue::types::Bucket::open_bucket("test")
        .map_err(|e| e.trace())
        .expect("failed to open empty bucket");
    let counter_key = String::from("counter");
    eprintln!("call `wasi:keyvalue/atomic.increment`...");
    let value = keyvalue::atomic::increment(&bucket, &counter_key, 1)
        .map_err(|e| e.trace())
        .expect("failed to increment `counter`");
    assert_eq!(value, 1);
    eprintln!("call `wasi:keyvalue/atomic.increment`...");
    let value = keyvalue::atomic::increment(&bucket, &counter_key, 41)
        .map_err(|e| e.trace())
        .expect("failed to increment `counter`");
    assert_eq!(value, 42);

    eprintln!("call `wasi:keyvalue/atomic.compare-and-swap`...");
    assert!(
        keyvalue::atomic::compare_and_swap(&bucket, &counter_key, 42, 4242)
            .expect("failed to compare and swap")
    );
    eprintln!("call `wasi:keyvalue/atomic.compare-and-swap`...");
    assert!(
        !keyvalue::atomic::compare_and_swap(&bucket, &counter_key, 42, 4242)
            .expect("failed to compare and swap")
    );
    eprintln!("call `wasi:keyvalue/atomic.compare-and-swap`...");
    assert!(
        keyvalue::atomic::compare_and_swap(&bucket, &counter_key, 4242, 42)
            .expect("failed to compare and swap")
    );
}

pub fn run_eventual_test(body: &Vec<u8>) {
    let bucket = keyvalue::types::Bucket::open_bucket("test")
        .map_err(|e| e.trace())
        .expect("failed to open empty bucket");
    let foo_key = String::from("foo");
    eprintln!("call `wasi:keyvalue/eventual.exists`...");
    keyvalue::eventual::exists(&bucket, &foo_key)
        .map_err(|e| e.trace())
        .expect("failed to check whether `foo` exists")
        .then_some(())
        .expect("`foo` does not exist");

    eprintln!("call `wasi:keyvalue/eventual.get`...");
    let foo_value = keyvalue::eventual::get(&bucket, &foo_key)
        .map_err(|e| e.trace())
        .expect("failed to get `foo`")
        .expect("`foo` does not exist in bucket");
    assert!(foo_value.incoming_value_size().is_err());

    let foo_value = keyvalue::types::IncomingValue::incoming_value_consume_sync(foo_value)
        .map_err(|e| e.trace())
        .expect("failed to get incoming value buffer");
    assert_eq!(foo_value, b"bar");

    eprintln!("call `wasi:keyvalue/eventual.get`...");
    let foo_value = keyvalue::eventual::get(&bucket, &foo_key)
        .map_err(|e| e.trace())
        .expect("failed to get `foo`")
        .expect("`foo` does not exist in bucket");
    let mut foo_stream = keyvalue::types::IncomingValue::incoming_value_consume_async(foo_value)
        .map_err(|e| e.trace())
        .expect("failed to get incoming value stream");
    let mut foo_value = vec![];
    let n = InputStreamReader::from(&mut foo_stream)
        .read_to_end(&mut foo_value)
        .expect("failed to read value from keyvalue input stream");
    assert_eq!(n, 3);
    assert_eq!(foo_value, b"bar");

    eprintln!("call `wasi:keyvalue/eventual.delete`...");
    keyvalue::eventual::delete(&bucket, &foo_key)
        .map_err(|e| e.trace())
        .expect("failed to delete `foo`");

    eprintln!("call `wasi:keyvalue/eventual.exists`...");
    let foo_exists = keyvalue::eventual::exists(&bucket, &foo_key)
        .map_err(|e| e.trace())
        .expect(
            "`exists` method should not have returned an error for `foo` key, which was deleted",
        );
    assert!(!foo_exists);

    let result_key = String::from("result");

    let result_value = keyvalue::types::OutgoingValue::new_outgoing_value();
    result_value
        .outgoing_value_write_body_sync(body)
        .expect("failed to write outgoing value");
    keyvalue::eventual::set(&bucket, &result_key, &result_value)
        .map_err(|e| e.trace())
        .expect("failed to set `result`");

    eprintln!("call `wasi:keyvalue/eventual.get`...");
    let result_value = keyvalue::eventual::get(&bucket, &result_key)
        .map_err(|e| e.trace())
        .expect("failed to get `result`")
        .expect("`result` does not exist in bucket");
    let result_value = keyvalue::types::IncomingValue::incoming_value_consume_sync(result_value)
        .map_err(|e| e.trace())
        .expect("failed to get incoming value buffer");
    assert_eq!(&result_value, body, "expected body, got {result_value:?}");

    let result_value = keyvalue::types::OutgoingValue::new_outgoing_value();
    let mut result_stream = result_value
        .outgoing_value_write_body_async()
        .expect("failed to get outgoing value output stream");
    let mut result_stream_writer = OutputStreamWriter::from(&mut result_stream);
    result_stream_writer
        .write_all(body)
        .expect("failed to write result to keyvalue output stream");
    result_stream_writer
        .flush()
        .expect("failed to flush keyvalue output stream");

    eprintln!("call `wasi:keyvalue/eventual.set`...");
    keyvalue::eventual::set(&bucket, &result_key, &result_value)
        .map_err(|e| e.trace())
        .expect("failed to set `result`");
}
