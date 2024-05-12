use wasmcloud_component::wasi::keyvalue;

pub fn run_atomics_test() {
    let bucket = keyvalue::store::open("test").expect("failed to open empty bucket");
    let counter_key = String::from("counter");
    eprintln!("call `wasi:keyvalue/atomics.increment`...");
    let value = keyvalue::atomics::increment(&bucket, &counter_key, 1)
        .expect("failed to increment `counter`");
    assert_eq!(value, 1);
    eprintln!("call `wasi:keyvalue/atomics.increment`...");
    let value = keyvalue::atomics::increment(&bucket, &counter_key, 41)
        .expect("failed to increment `counter`");
    assert_eq!(value, 42);
}

pub fn run_store_test(body: &Vec<u8>) {
    let bucket = keyvalue::store::open("test").expect("failed to open empty bucket");
    let foo_key = String::from("foo");

    eprintln!("call `wasi:keyvalue/store.list-keys`...");
    let keyvalue::store::KeyResponse { keys, cursor } = bucket
        .list_keys(None)
        .expect("failed to list keys");
    assert_eq!(keys, ["foo"]);
    assert_eq!(cursor, None);

    eprintln!("call `wasi:keyvalue/store.exists`...");
    bucket
        .exists(&foo_key)
        .expect("failed to check whether `foo` exists")
        .then_some(())
        .expect("`foo` does not exist");

    eprintln!("call `wasi:keyvalue/store.get`...");
    let foo_value = bucket
        .get(&foo_key)
        .expect("failed to get `foo`")
        .expect("`foo` does not exist in bucket");
    assert_eq!(foo_value, b"bar");

    eprintln!("call `wasi:keyvalue/store.delete`...");
    bucket.delete(&foo_key).expect("failed to delete `foo`");

    eprintln!("call `wasi:keyvalue/store.exists`...");
    let foo_exists = bucket.exists(&foo_key).expect(
        "`exists` method should not have returned an error for `foo` key, which was deleted",
    );
    assert!(!foo_exists);

    eprintln!("call `wasi:keyvalue/store.get`...");
    let foo_value = bucket.get(&foo_key).expect("failed to get `foo`");
    assert_eq!(foo_value, None);

    eprintln!("call `wasi:keyvalue/store.list-keys`...");
    let keyvalue::store::KeyResponse { keys, cursor } = bucket
        .list_keys(None)
        .expect("failed to list keys");
    assert_eq!(keys, [""; 0]);
    assert_eq!(cursor, None);

    let result_key = String::from("result");

    eprintln!("call `wasi:keyvalue/store.set`...");
    bucket
        .set(&result_key, body)
        .expect("failed to set `result`");

    eprintln!("call `wasi:keyvalue/store.get`...");
    let result_value = bucket
        .get(&result_key)
        .expect("failed to get `result`")
        .expect("`result` does not exist in bucket");
    assert_eq!(&result_value, body, "expected body, got {result_value:?}");

    eprintln!("call `wasi:keyvalue/store.set`...");
    bucket
        .set(&result_key, &result_value)
        .expect("failed to set `result`");
}
