use url::Url;
use wasmcloud_component::{
    http::{ErrorCode, IncomingRequest, OutgoingBody, Response, Server},
    wasi::{
        self,
        http::types::{Fields, Scheme},
    },
};

#[derive(serde::Deserialize)]
struct DogResponse {
    message: String,
}

struct DogFetcher;

impl Server for DogFetcher {
    fn handle(_request: IncomingRequest) -> Result<Response<impl OutgoingBody>, ErrorCode> {
        // Get dog picture URL
        let dog_picture_url = make_outgoing_request("https://dog.ceo/api/breeds/image/random")?;
        let dog_response: DogResponse =
            serde_json::from_reader(dog_picture_url.stream().map_err(|_| {
                ErrorCode::InternalError(Some("failed to stream dog API response".to_string()))
            })?)
            .map_err(|_| {
                ErrorCode::InternalError(Some("failed to deserialize dog API response".to_string()))
            })?;

        // Get dog picture
        let dog_picture = make_outgoing_request(&dog_response.message)?;
        // TODO: blobstore
        Ok(Response::new(dog_picture))
    }
}

fn make_outgoing_request(url: &str) -> Result<wasi::http::types::IncomingBody, ErrorCode> {
    let req = wasi::http::outgoing_handler::OutgoingRequest::new(Fields::new());
    let url = Url::parse(url)
        .map_err(|_| ErrorCode::InternalError(Some("failed to parse URL".to_string())))?;

    if url.scheme() == "https" {
        req.set_scheme(Some(&Scheme::Https)).map_err(|_| {
            ErrorCode::InternalError(Some("failed to set HTTPS scheme".to_string()))
        })?;
    } else if url.scheme() == "http" {
        req.set_scheme(Some(&Scheme::Http))
            .map_err(|_| ErrorCode::InternalError(Some("failed to set HTTP scheme".to_string())))?;
    } else {
        req.set_scheme(Some(&Scheme::Other(url.scheme().to_string())))
            .map_err(|_| {
                ErrorCode::InternalError(Some("failed to set custom scheme".to_string()))
            })?;
    }

    req.set_authority(Some(url.authority()))
        .map_err(|_| ErrorCode::InternalError(Some("failed to set URL authority".to_string())))?;

    req.set_path_with_query(Some(url.path()))
        .map_err(|_| ErrorCode::InternalError(Some("failed to set URL path".to_string())))?;

    match wasi::http::outgoing_handler::handle(req, None) {
        Ok(resp) => {
            resp.subscribe().block();
            let response = resp
                .get()
                .ok_or(())
                .map_err(|_| {
                    ErrorCode::InternalError(Some("HTTP request response missing".to_string()))
                })?
                .map_err(|_| {
                    ErrorCode::InternalError(Some(
                        "HTTP request response requested more than once".to_string(),
                    ))
                })?
                .map_err(|_| ErrorCode::InternalError(Some("HTTP request failed".to_string())))?;

            response.consume().map_err(|_| {
                ErrorCode::InternalError(Some("failed to consume response".to_string()))
            })
        }
        Err(e) => Err(e),
    }
}

wasmcloud_component::http::export!(DogFetcher);
