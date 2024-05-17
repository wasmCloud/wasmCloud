#![allow(clippy::missing_safety_doc)]
wit_bindgen::generate!();

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

#[derive(serde::Deserialize)]
struct DogResponse {
    message: String,
}

struct DogFetcher;

impl Guest for DogFetcher {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let dog_picture_url = match futures::executor::block_on(
            reqwest::Client::new()
                .get("https://dog.ceo/api/breeds/image/random")
                .send(),
        ) {
            Ok(resp) => {
                let body =
                    futures::executor::block_on(resp.bytes()).expect("should have response bytes");
                let dog_response: DogResponse = serde_json::from_slice(&body).unwrap();
                dog_response.message
            }
            Err(e) => {
                format!("Got error when trying to fetch dog: {}", e)
            }
        };
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let response_body = response.body().unwrap();
        let write_stream = response_body.write().unwrap();

        // Write the headers and then write the body
        ResponseOutparam::set(response_out, Ok(response));
        // For simplicity, we're just writing this string out. However, when the body gets long
        // enough (generally >4096 bytes), you'll need to loop and write chunks of the body.
        write_stream
            .blocking_write_and_flush(dog_picture_url.as_bytes())
            .unwrap();
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
    }
}

export!(DogFetcher);
