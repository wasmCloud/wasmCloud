use std::io::Write;

use wasmcloud_actor::wasi::blobstore;
use wasmcloud_actor::OutputStreamWriter;

fn assert_create_container(name: &String, min_created_at: u64) -> blobstore::container::Container {
    eprintln!("call `wasi:blobstore/blobstore.create-container`...");
    let container =
        blobstore::blobstore::create_container(name).expect("failed to create container");
    eprintln!("call `wasi:blobstore/blobstore.container-exists`...");
    assert_eq!(
        &container.name().expect("failed to get container name"),
        name,
    );
    let md = container
        .info()
        .expect("failed to get info of created container");
    assert_eq!(&md.name, name);
    assert!(md.created_at >= min_created_at);

    assert!(blobstore::blobstore::container_exists(name)
        .expect("failed to check whether container exists"));
    eprintln!("call `wasi:blobstore/container.container.info`...");
    {
        eprintln!("call `wasi:blobstore/blobstore.get-container`...");
        let container = blobstore::blobstore::get_container(name).expect("failed to get container");
        eprintln!("call `wasi:blobstore/container.container.info`...");
        let info = container
            .info()
            .expect("failed to get info of got container");
        assert_eq!(info.name, md.name);
        assert_eq!(info.created_at, md.created_at);
    }
    {
        eprintln!("call `wasi:blobstore/blobstore.create-container`...");
        let container = blobstore::blobstore::create_container(name)
            .expect("failed to create existing container");
        eprintln!("call `wasi:blobstore/container.container.info`...");
        let info = container
            .info()
            .expect("failed to get info of created container");
        assert_eq!(info.name, md.name);
        assert_eq!(info.created_at, md.created_at);
    }
    container
        .clear()
        .expect("failed to clear an empty container");
    eprintln!("call `wasi:blobstore/container.container.list-objects` on an empty container...");
    let objects = container
        .list_objects()
        .expect("failed to list objects in an empty container");
    eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names` on an empty container..."
        );
    let (names, end) = objects
        .read_stream_object_names(10)
        .expect("failed to read object names from an empty container");
    assert!(end);
    assert!(names.is_empty());
    container
}

fn assert_write_container_data(
    container: &blobstore::container::Container,
    key: &String,
    data: &[u8],
) {
    let value = blobstore::types::OutgoingValue::new_outgoing_value();
    let mut value_stream = value
        .outgoing_value_write_body()
        .expect("failed to get outgoing value output stream");
    let mut value_stream_writer = OutputStreamWriter::from(&mut value_stream);
    eprintln!("write body to outgoing blobstore stream...");
    value_stream_writer
        .write_all(data)
        .expect("failed to write result to blobstore output stream");
    eprintln!("flush outgoing blobstore stream...");
    value_stream_writer
        .flush()
        .expect("failed to flush blobstore output stream");
    eprintln!("call `wasi:blobstore/container.container.write-data`...");
    container
        .write_data(key, &value)
        .expect("failed to write data");
    eprintln!("call `wasi:blobstore/container.container.get-data`...");
    let stored_value = container
        .get_data(
            key,
            0,
            data.len().saturating_add(10).try_into().unwrap_or(u64::MAX),
        )
        .expect("failed to get container data");
    let stored_value = blobstore::types::IncomingValue::incoming_value_consume_sync(stored_value)
        .expect("failed to get stored value buffer");
    assert_eq!(stored_value, data);
}

pub fn run_test(min_created_at: u64, body: &[u8], name: impl Into<String>) {
    let name = name.into();

    eprintln!("create `{name}` container");
    let container = assert_create_container(&name, min_created_at);

    eprintln!("call `wasi:blobstore/container.container.has-object`...");
    assert!(!container
        .has_object(&String::from("result"))
        .expect("failed to check whether `result` object exists"));
    eprintln!("call `wasi:blobstore/container.container.delete-object`...");
    container
        .delete_object(&String::from("result"))
        .expect("failed to delete object");

    eprintln!("write `foo` object...");
    assert_write_container_data(&container, &String::from("foo"), b"foo");

    eprintln!("rewrite `foo` object...");
    assert_write_container_data(&container, &String::from("foo"), b"newfoo");

    eprintln!("write `bar` object...");
    assert_write_container_data(&container, &String::from("bar"), b"bar");

    eprintln!("write `baz` object...");
    assert_write_container_data(&container, &String::from("baz"), b"baz");

    eprintln!("write `result` object...");
    assert_write_container_data(&container, &String::from("result"), body);

    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut first_names, end) = objects
            .read_stream_object_names(2)
            .expect("failed to read object names");
        assert!(!end);

        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);

        names.append(&mut first_names);
        names.sort();
        assert_eq!(names, ["bar", "baz", "foo", "result"]);
    }

    let name_other = format!("{name}-other");
    eprintln!("create `{name_other}` container");
    let other = assert_create_container(&name_other, min_created_at);

    let container_foo = blobstore::types::ObjectId {
        container: name.clone(),
        object: String::from("foo"),
    };

    let other_foobar = blobstore::types::ObjectId {
        container: name_other.clone(),
        object: String::from("foobar"),
    };

    eprintln!("call `wasi:blobstore/blobstore.move-object`...");
    blobstore::blobstore::move_object(&other_foobar, &container_foo)
        .expect_err("should not be possible to move non-existing object");

    eprintln!("call `wasi:blobstore/blobstore.move-object`...");
    blobstore::blobstore::move_object(&container_foo, &other_foobar)
        .expect("failed to move object");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        names.sort();
        assert_eq!(names, ["bar", "baz", "result"]);
    }
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = other
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(2)
            .expect("failed to read object names");
        assert!(end);
        assert_eq!(names, ["foobar"])
    };
    eprintln!("call `wasi:blobstore/container.container.get-data`...");
    let value = other
        .get_data(&String::from("foobar"), 3, 10)
        .expect("failed to get `foobar` object from `other` container");
    let value = blobstore::types::IncomingValue::incoming_value_consume_sync(value)
        .expect("failed to get stored value buffer");
    assert_eq!(value, b"foo");

    eprintln!("call `wasi:blobstore/blobstore.copy-object`...");
    blobstore::blobstore::copy_object(&container_foo, &other_foobar)
        .expect_err("should not be possible to copy a non-existing object");

    eprintln!("call `wasi:blobstore/blobstore.copy-object`...");
    blobstore::blobstore::copy_object(&other_foobar, &container_foo)
        .expect("failed to copy object");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = other
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(2)
            .expect("failed to read object names");
        assert!(end);
        assert_eq!(names, ["foobar"]);
    }
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        names.sort();
        assert_eq!(names, ["bar", "baz", "foo", "result"]);
    }
    eprintln!("call `wasi:blobstore/container.container.get-data`...");
    let value = container
        .get_data(&String::from("foo"), 0, 9999)
        .expect("failed to get `foobar` object from `other` container");
    let value = blobstore::types::IncomingValue::incoming_value_consume_sync(value)
        .expect("failed to get stored value buffer");
    assert_eq!(value, b"newfoo");

    container.clear().expect("failed to clear container");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        assert!(names.is_empty());
    }
    container
        .delete_object(&String::from("foo"))
        .expect("failed to delete non-existing object");
    eprintln!("call `wasi:blobstore/blobstore.delete-container`...");
    blobstore::blobstore::delete_container(&name).expect("failed to delete container");
    eprintln!("call `wasi:blobstore/blobstore.get-container`...");
    blobstore::blobstore::get_container(&name).expect_err("container should have been deleted");
    eprintln!("call `wasi:blobstore/blobstore.container-exists`...");
    assert!(
        !blobstore::blobstore::container_exists(&name).expect("container should have been deleted")
    );

    eprintln!("call `wasi:blobstore/blobstore.delete-object`...");
    other
        .delete_object(&String::from("foobar"))
        .expect("failed to delete object");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = other
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        assert!(names.is_empty());
    }
    eprintln!("call `wasi:blobstore/blobstore.delete-container`...");
    blobstore::blobstore::delete_container(&name_other).expect("failed to delete container");
    eprintln!("call `wasi:blobstore/blobstore.get-container`...");
    blobstore::blobstore::get_container(&name_other)
        .expect_err("container should have been deleted");
    eprintln!("call `wasi:blobstore/blobstore.container-exists`...");
    assert!(!blobstore::blobstore::container_exists(&name_other)
        .expect("container should have been deleted"));
}
