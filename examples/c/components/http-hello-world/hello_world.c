// This example borrows heavily from https://gist.github.com/ac000/906707af37b42cdcc9d3cc4c2715da99#file-002-luw-echo-request-c-L1

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include "hello.h"


#define MAX_PATH 1024
#define MAX_READ_BYTES (8 * 1024 * 1024)
#define BUFFER_SIZE 500000

#define BUF_ADD(fmt, ...) \
	fprintf(out, fmt, ##__VA_ARGS__);

static const struct {
	const char *method;
} http_method_map[] = {
	[WASI_HTTP_TYPES_METHOD_GET]	 = { "GET"     },
	[WASI_HTTP_TYPES_METHOD_HEAD]	 = { "HEAD"    },
	[WASI_HTTP_TYPES_METHOD_POST]    = { "POST"    },
	[WASI_HTTP_TYPES_METHOD_PUT]	 = { "PUT"     },
	[WASI_HTTP_TYPES_METHOD_DELETE]	 = { "DELETE"  },
	[WASI_HTTP_TYPES_METHOD_CONNECT] = { "CONNECT" },
	[WASI_HTTP_TYPES_METHOD_OPTIONS] = { "OPTIONS" },
	[WASI_HTTP_TYPES_METHOD_TRACE]	 = { "TRACE"   },
	[WASI_HTTP_TYPES_METHOD_PATCH]   = { "PATCH"   },
	[WASI_HTTP_TYPES_METHOD_OTHER]	 = { "OTHER"   }
};

void exports_wasi_http_incoming_handler_handle(
	exports_wasi_http_incoming_handler_own_incoming_request_t request,
	exports_wasi_http_incoming_handler_own_response_outparam_t response_out)
{
	FILE *out;
	size_t size;
	char *out_ptr;
	char *ptr;
	char clen[32];
	wasi_http_types_borrow_incoming_request_t b_req;
	wasi_http_types_own_headers_t hdrs;
	wasi_http_types_borrow_fields_t b_hdrs;
	wasi_http_types_own_incoming_body_t r_body;
	wasi_http_types_borrow_incoming_body_t b_r_body;
	wasi_http_types_own_outgoing_response_t resp;
	wasi_http_types_borrow_outgoing_response_t b_resp;
	wasi_http_types_own_fields_t fields;
	wasi_http_types_borrow_fields_t b_fields;
	wasi_http_types_own_outgoing_body_t body;
	wasi_http_types_borrow_outgoing_body_t b_body;
	wasi_io_streams_own_output_stream_t out_stream;
	wasi_io_streams_borrow_output_stream_t b_out_stream;
	hello_list_u8_t stream_data;
	wasi_io_streams_stream_error_t stream_err;
	wasi_http_types_header_error_t hdr_err;
	wasi_http_types_field_key_t key;
	wasi_http_types_field_value_t value;
	wasi_http_types_method_t method;
	hello_list_tuple2_field_key_field_value_t fvk;
	wasi_http_types_own_input_stream_t in_stream;
	wasi_io_streams_borrow_input_stream_t b_in_stream;
	hello_list_u8_t data;
	wasi_io_streams_stream_error_t in_stream_err;
	hello_string_t prstr;
	size_t content_length;
	bool ok;

	b_req = wasi_http_types_borrow_incoming_request(request);

	out = open_memstream(&out_ptr, &size);

	BUF_ADD("*** WasmCloud with C ***\n\n");

	BUF_ADD("[Request Info]\n");
	wasi_http_types_method_incoming_request_path_with_query(b_req, &prstr);
	BUF_ADD("REQUEST_PATH = %.*s\n", (int)prstr.len, prstr.ptr);
	wasi_http_types_method_incoming_request_method(b_req, &method);
	BUF_ADD("METHOD       = %s\n", http_method_map[method.tag].method);
	ptr = memchr(prstr.ptr, '?', prstr.len);
	BUF_ADD("QUERY        = %.*s\n",
	 ptr ? (int)(((char *)(prstr.ptr + prstr.len)) - ptr - 1) : 0,
	 ptr ? ptr + 1 : "");

	BUF_ADD("\n[Request Headers]\n");
	hdrs = wasi_http_types_method_incoming_request_headers(b_req);
	b_hdrs = wasi_http_types_borrow_fields(hdrs);

	wasi_http_types_method_fields_entries(b_hdrs, &fvk);
	for (size_t i = 0; i < fvk.len; i++) {
		BUF_ADD("%.*s = %.*s\n",
	  (int)fvk.ptr[i].f0.len, fvk.ptr[i].f0.ptr,
	  (int)fvk.ptr[i].f1.len, fvk.ptr[i].f1.ptr);
		if (fvk.ptr[i].f0.len == 14 && strncasecmp((const char *)fvk.ptr[i].f0.ptr, "Content-Length", 14) == 0) {
			content_length = atoll((const char *)fvk.ptr[i].f1.ptr);
		}
	}

	hello_list_tuple2_field_key_field_value_free(&fvk);

	wasi_http_types_method_incoming_request_consume(b_req, &r_body);
	b_r_body = wasi_http_types_borrow_incoming_body(r_body);
	wasi_http_types_method_incoming_body_stream(b_r_body, &in_stream);
	b_in_stream = wasi_io_streams_borrow_input_stream(in_stream);

	data.ptr = malloc(content_length);
	if (data.ptr == NULL) {
		fprintf(stderr, "Memory allocation failed for content length: %zu\n", content_length);
	}
	data.len = 0;
	while (data.len < content_length) {
		size_t bytes_to_read = content_length - data.len;
		if (bytes_to_read > MAX_READ_BYTES) {
			bytes_to_read = MAX_READ_BYTES;
		}

		hello_list_u8_t temp_data = {0};
		ok = wasi_io_streams_method_input_stream_blocking_read(b_in_stream, bytes_to_read, &temp_data, &in_stream_err);
		if (!ok) {
			BUF_ADD("Error reading from stream: %d\n", in_stream_err.tag);
			free(data.ptr);
			break;
		}
		if (temp_data.len == 0) {
			// Unexpected end of stream
			BUF_ADD("Stream ended prematurely. Expected %zu bytes, got %zu\n", content_length, data.len);
			free(data.ptr);
			break;
		}

		// Copy temp_data to data
		memcpy(data.ptr + data.len, temp_data.ptr, temp_data.len);
		data.len += temp_data.len;

		hello_list_u8_free(&temp_data);
	}

	if (method.tag == WASI_HTTP_TYPES_METHOD_POST || method.tag == WASI_HTTP_TYPES_METHOD_PUT) {
		BUF_ADD("\n[%s data]\n", http_method_map[method.tag].method);
		BUF_ADD("%.*s\n", (int)data.len, data.ptr);
		if (data.len != content_length) {
			BUF_ADD("\nExpected content of length %zu, got %zu\n", content_length, data.len);
		}
	}

	fclose(out);

	fields = wasi_http_types_constructor_fields();
	b_fields = wasi_http_types_borrow_fields(fields); 

	hello_string_set((hello_string_t *)&key, "Content-Length");
	sprintf(clen, "%zu", size);
	hello_string_set((hello_string_t *)&value, clen);

	wasi_http_types_method_fields_append(b_fields, &key, &value, &hdr_err);

	resp = wasi_http_types_constructor_outgoing_response(fields);

	b_resp = wasi_http_types_borrow_outgoing_response(resp);
	wasi_http_types_method_outgoing_response_body(b_resp, &body);
	b_body = wasi_http_types_borrow_outgoing_body(body);

	wasi_http_types_method_outgoing_body_write(b_body, &out_stream);
	b_out_stream = wasi_io_streams_borrow_output_stream(out_stream);

	stream_data.len = size;
	stream_data.ptr = (uint8_t *)out_ptr;
	ok = wasi_io_streams_method_output_stream_blocking_write_and_flush( b_out_stream, &stream_data, &stream_err);

	free(out_ptr);

	wasi_http_types_result_own_outgoing_response_error_code_t result = {
		.is_err = false,
		.val = {
			.ok = resp
		}
	};
	wasi_http_types_static_response_outparam_set(response_out, &result);
}
