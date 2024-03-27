use std::io::{Read, Write};

use wasmcloud_actor::wasi::http;
use wasmcloud_actor::wasi::io::poll::poll;
use wasmcloud_actor::{InputStreamReader, OutputStreamWriter};

fn assert_http_echo(
    request: http::types::IncomingRequest,
    response_out: http::types::ResponseOutparam,
) {
    assert!(matches!(request.method(), http::types::Method::Put));

    let response = http::types::OutgoingResponse::new(http::types::Fields::new());
    let response_body = response
        .body()
        .expect("failed to get outgoing response body");
    let request_body = request
        .consume()
        .expect("failed to get incoming request body");
    http::types::ResponseOutparam::set(response_out, Ok(response));
    {
        let input_stream = request_body
            .stream()
            .expect("failed to get incoming request stream");
        let output_stream = response_body
            .write()
            .expect("failed to get outgoing response stream");

        eprintln!("[echo] read `t`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `t`"),
            b"t"
        );

        eprintln!("[echo] write `t`...");
        output_stream
            .blocking_write_and_flush(b"t")
            .expect("failed to write `t`");

        eprintln!("[echo] splice `es`...");
        let n = output_stream
            .blocking_splice(&input_stream, 2)
            .expect("failed to splice");
        assert_eq!(n, 2);

        eprintln!("[echo] read `t`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `t`"),
            b"t"
        );

        eprintln!("[echo] write `t`...");
        output_stream
            .blocking_write_and_flush(b"t")
            .expect("failed to write `t`");

        eprintln!("[echo] read `i`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `i`"),
            b"i"
        );
        eprintln!("[echo] write `i`...");
        output_stream
            .blocking_write_and_flush(b"i")
            .expect("failed to write `i`");

        eprintln!("[echo] read `n`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `n`"),
            b"n"
        );
        eprintln!("[echo] read `g`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `g`"),
            b"g"
        );
        eprintln!("[echo] write `ng`...");
        output_stream
            .blocking_write_and_flush(b"ng")
            .expect("failed to write `ng`");
        assert!(input_stream.blocking_read(1).is_err());
    };
    let _trailers = http::types::IncomingBody::finish(request_body);
    http::types::OutgoingBody::finish(response_body, None).expect("failed to finish response body");
}

fn assert_http_run(
    request: http::types::IncomingRequest,
    response_out: http::types::ResponseOutparam,
) {
    assert!(matches!(request.method(), http::types::Method::Post));
    assert_eq!(request.path_with_query().as_deref(), Some("/foo?bar=baz"));
    assert!(matches!(request.scheme(), Some(http::types::Scheme::Http)));
    // NOTE: This will be validated after the body has been received
    let request_authority = request.authority().expect("authority missing");
    let headers = request.headers();

    let mut header_entries = headers.entries();
    header_entries.sort();
    let mut header_iter = header_entries.clone().into_iter();

    assert_eq!(header_iter.next(), Some(("accept".into(), b"*/*".to_vec())));
    assert_eq!(headers.get(&String::from("accept")), vec![b"*/*"]);

    let (content_length_key, content_length_value) =
        header_iter.next().expect("`content-length` header missing");
    assert_eq!(content_length_key, "content-length");
    assert_eq!(
        headers.get(&String::from("content-length")),
        vec![content_length_value.as_slice()]
    );
    let content_length: usize = String::from_utf8(content_length_value)
        .expect("`content-length` value is not a valid string")
        .parse()
        .expect("`content-length` value is not a valid usize");

    assert_eq!(
        header_iter.next(),
        Some(("test-header".into(), b"test-value".to_vec()))
    );
    assert_eq!(
        headers.get(&String::from("test-header")),
        vec![b"test-value"]
    );
    assert!(header_iter.next().is_none());

    let headers_clone = headers.clone();
    headers_clone
        .set(&String::from("foo"), &[b"bar".to_vec()])
        .expect("failed to set `foo` header");
    headers_clone
        .append(&String::from("foo"), &b"baz".to_vec())
        .expect("failed to append `foo` header");
    assert_eq!(
        headers_clone.get(&String::from("foo")),
        vec![b"bar", b"baz"]
    );
    headers_clone
        .delete(&String::from("foo"))
        .expect("failed to delete `foo` header");
    let mut headers_clone = headers_clone.entries();
    headers_clone.sort();
    assert_eq!(headers_clone, header_entries);

    let request_body = request
        .consume()
        .expect("failed to get incoming request body");
    let (body, authority) = {
        let mut buf = vec![];
        let mut stream = request_body
            .stream()
            .expect("failed to get incoming request stream");
        InputStreamReader::from(&mut stream)
            .read_to_end(&mut buf)
            .expect("failed to read value from incoming request stream");
        assert_eq!(buf.len(), content_length);
        crate::run_test(&buf)
    };
    let _trailers = http::types::IncomingBody::finish(request_body);

    assert_eq!(request_authority, authority);

    let outgoing_request = http::types::OutgoingRequest::new(http::types::Fields::new());
    outgoing_request
        .set_method(&http::types::Method::Put)
        .expect("failed to set request method");
    outgoing_request
        .set_path_with_query(Some("/echo"))
        .expect("failed to set request path with query");
    outgoing_request
        .set_scheme(Some(&http::types::Scheme::Http))
        .expect("failed to set request scheme");
    outgoing_request
        .set_authority(Some(&authority))
        .expect("failed to set request authority");
    let outgoing_request_body = outgoing_request
        .body()
        .expect("failed to get outgoing request body");
    let outgoing_request_response = http::outgoing_handler::handle(outgoing_request, None)
        .expect("failed to handle HTTP request");
    let outgoing_request_response_sub = outgoing_request_response.subscribe();

    eprintln!("poll outgoing HTTP request response...");
    assert_eq!(poll(&[&outgoing_request_response_sub]), [0]);
    let outgoing_request_response = outgoing_request_response
        .get()
        .expect("HTTP request response missing")
        .expect("HTTP request response requested more than once")
        .expect("HTTP request failed");
    assert_eq!(outgoing_request_response.status(), 200);

    // TODO: Assert headers
    _ = outgoing_request_response.headers();
    let outgoing_request_response_body = outgoing_request_response
        .consume()
        .expect("failed to get incoming request body");
    {
        let input_stream = outgoing_request_response_body
            .stream()
            .expect("failed to get HTTP request response stream");
        let output_stream = outgoing_request_body
            .write()
            .expect("failed to get outgoing request stream");

        eprintln!("write `t`...");
        output_stream
            .blocking_write_and_flush(b"t")
            .expect("failed to write `t`");
        eprintln!("read `t`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `t`"),
            b"t"
        );

        eprintln!("write `est`...");
        output_stream
            .blocking_write_and_flush(b"est")
            .expect("failed to write `est`");
        eprintln!("read `e`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `e`"),
            b"e"
        );
        eprintln!("read `s`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `s`"),
            b"s"
        );
        eprintln!("read `t`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `t`"),
            b"t"
        );

        eprintln!("write `i`...");
        output_stream
            .blocking_write_and_flush(b"i")
            .expect("failed to write `i`");
        eprintln!("read `i`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `i`"),
            b"i"
        );

        eprintln!("write `ng`...");
        output_stream
            .blocking_write_and_flush(b"ng")
            .expect("failed to write `ng`");
        eprintln!("read `n`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `n`"),
            b"n"
        );
        eprintln!("read `g`...");
        assert_eq!(
            input_stream.blocking_read(1).expect("failed to read `g`"),
            b"g"
        );
    }
    eprintln!("set response");
    http::types::OutgoingBody::finish(outgoing_request_body, None)
        .expect("failed to finish sending request body");
    // TODO: Assert trailers
    let _trailers = http::types::IncomingBody::finish(outgoing_request_response_body);

    let response = http::types::OutgoingResponse::new(http::types::Fields::new());
    let response_body = response
        .body()
        .expect("failed to get outgoing response body");
    {
        let mut stream = response_body
            .write()
            .expect("failed to get outgoing response stream");
        let mut w = OutputStreamWriter::from(&mut stream);
        w.write_all(&body)
            .expect("failed to write body to outgoing response stream");
        w.flush().expect("failed to flush outgoing response stream");
    }
    http::types::OutgoingBody::finish(response_body, None).expect("failed to finish response body");

    http::types::ResponseOutparam::set(response_out, Ok(response));
}

impl crate::exports::wasi::http::incoming_handler::Guest for crate::Actor {
    fn handle(request: http::types::IncomingRequest, response_out: http::types::ResponseOutparam) {
        let path_with_query = request.path_with_query();
        if path_with_query.as_deref() == Some("/echo") {
            assert_http_echo(request, response_out);
        } else {
            assert_http_run(request, response_out)
        }
    }
}
