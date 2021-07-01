mod httpserver;
pub use httpserver::*;

impl Default for HttpResponse {
    /// Create default response with status 200(OK), empty headers, empty body
    fn default() -> Self {
        HttpResponse {
            status_code: 200,
            status: "OK".to_string(),
            header: Default::default(),
            body: Vec::default(),
        }
    }
}
