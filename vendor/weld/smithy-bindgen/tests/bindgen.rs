#[test]
fn test_str() {
    mod test {
        smithy_bindgen::smithy_bindgen!(
            "keyvalue/keyvalue.smithy",
            "org.wasmcloud.interface.keyvalue"
        );
    }

    use test::ListAddRequest;
    use test::StringList;

    let _x = StringList::new();
    let _y = ListAddRequest::default();
    println!("hello");
}

#[test]
fn test_path_base() {
    mod test {
        smithy_bindgen::smithy_bindgen!(
            { path: ".", files: [ "tests/test-bindgen.smithy"]},
             "org.wasmcloud.test.bindgen");
    }

    use test::Thing;

    let x = Thing { value: "hello".into() };
    println!("{}", x.value);
}

#[test]
fn test_path_complete() {
    mod test {
        smithy_bindgen::smithy_bindgen!(
            { path: "./tests/test-bindgen.smithy"},
             "org.wasmcloud.test.bindgen");
    }

    use test::Thing;

    let x = Thing { value: "hello".into() };
    println!("{}", x.value);
}
