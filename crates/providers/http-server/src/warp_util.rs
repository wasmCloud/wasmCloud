use std::collections::HashMap;
use std::convert::Infallible;

use tracing::error;
use warp::filters::cors::Builder;
use warp::Filter;

use crate::settings::ServiceSettings;
use crate::HttpServerError;

/// Convert request headers from incoming warp server to HeaderMap
pub(crate) fn convert_request_headers(headers: &http::HeaderMap) -> HashMap<String, Vec<String>> {
    let mut hmap = HashMap::default();
    for k in headers.keys() {
        let vals = headers
            .get_all(k)
            .iter()
            // from http crate:
            //    In practice, HTTP header field values are usually valid ASCII.
            //     However, the HTTP spec allows for a header value to contain
            //     opaque bytes as well.
            // This implementation only forwards headers with ascii values to the actor.
            .filter_map(|val| val.to_str().ok())
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if !vals.is_empty() {
            hmap.insert(k.to_string(), vals);
        }
    }
    hmap
}

/// Convert HeaderMap from actor into warp's HeaderMap for returning to http client
pub(crate) fn convert_response_headers(
    header: HashMap<String, Vec<String>>,
    headers_mut: &mut http::header::HeaderMap,
) {
    let map = headers_mut;
    for (k, vals) in header.into_iter() {
        let name = match http::header::HeaderName::from_bytes(k.as_bytes()) {
            Ok(name) => name,
            Err(e) => {
                error!(
                    header_name = %k,
                    error = %e,
                    "invalid response header name, sending without this header"
                );
                continue;
            }
        };
        map.remove(&name);
        for val in vals.into_iter() {
            let value = match http::header::HeaderValue::try_from(val) {
                Ok(value) => value,
                Err(e) => {
                    error!(
                        error = %e,
                        "Non-ascii header value, skipping this header"
                    );
                    continue;
                }
            };
            map.append(&name, value);
        }
    }
}

/// Get raw query as string or optional query
pub(crate) fn opt_raw_query() -> impl Filter<Extract = (String,), Error = Infallible> + Copy {
    warp::any().and(
        warp::filters::query::raw()
            .or(warp::any().map(String::default))
            .unify(),
    )
}

/// build warp Cors filter from settings
pub(crate) fn cors_filter(
    settings: &ServiceSettings,
) -> Result<warp::filters::cors::Cors, HttpServerError> {
    let mut cors: Builder = warp::cors();

    match settings.cors.allowed_origins {
        Some(ref allowed_origins) if !allowed_origins.is_empty() => {
            cors = cors.allow_origins(allowed_origins.iter().map(AsRef::as_ref));
        }
        _ => {
            cors = cors.allow_any_origin();
        }
    }

    if let Some(ref allowed_headers) = settings.cors.allowed_headers {
        cors = cors.allow_headers(allowed_headers.iter());
    }
    if let Some(ref allowed_methods) = settings.cors.allowed_methods {
        for m in allowed_methods.iter() {
            match http::method::Method::try_from(m.as_str()) {
                Err(_) => {
                    return Err(HttpServerError::InvalidParameter(format!(
                        "method: '{}'",
                        m
                    )))
                }
                Ok(method) => {
                    cors = cors.allow_method(method);
                }
            }
        }
    }

    if let Some(ref exposed_headers) = settings.cors.exposed_headers {
        cors = cors.expose_headers(exposed_headers.iter());
    }

    if let Some(max_age) = settings.cors.max_age_secs {
        cors = cors.max_age(std::time::Duration::from_secs(max_age));
    }
    Ok(cors.build())
}

#[cfg(test)]
mod tests {

    use crate::{HttpServerError, CONTENT_LEN_LIMIT, DEFAULT_MAX_CONTENT_LEN};

    /// Convert setting for max content length of form '[0-9]+(g|G|m|M|k|K)?'
    /// Empty string is accepted and returns the default value (currently '10M')
    pub fn convert_human_size(value: &str) -> Result<u64, HttpServerError> {
        let value = value.trim();
        let mut limit = None;
        if value.is_empty() {
            limit = Some(DEFAULT_MAX_CONTENT_LEN);
        } else if let Ok(num) = value.parse::<u64>() {
            limit = Some(num);
        } else {
            let (num, units) = value.split_at(value.len() - 1);
            if let Ok(base_value) = num.trim().parse::<u64>() {
                match units {
                    "k" | "K" => {
                        limit = Some(base_value * 1024);
                    }
                    "m" | "M" => {
                        limit = Some(base_value * 1024 * 1024);
                    }
                    "g" | "G" => {
                        limit = Some(base_value * 1024 * 1024 * 1024);
                    }
                    _ => {}
                }
            }
        }
        match limit {
            Some(x) if x > 0 && x <= CONTENT_LEN_LIMIT => Ok(x),
            Some(_) => {
                Err(HttpServerError::Settings(
                    format!(
                        "Invalid size in max_content_len '{value}': value must be >0 and <= {CONTENT_LEN_LIMIT}", 
                    )
                ))
            }
            None => {
                Err(HttpServerError::Settings(
                    format!(
                        "Invalid size in max_content_len: '{value}'. Should be a number, optionally followed by 'K', 'M', or 'G'. Example: '10M'. Value must be <= i32::MAX")
                ))
            }
        }
    }

    #[test]
    fn parse_max_content_len() {
        // emtpy string returns default
        assert_eq!(convert_human_size("").unwrap(), DEFAULT_MAX_CONTENT_LEN);
        // simple number
        assert_eq!(convert_human_size("4").unwrap(), 4);
        assert_eq!(convert_human_size("12345678").unwrap(), 12345678);
        // k, K, m, M, g, G suffix
        assert_eq!(convert_human_size("2k").unwrap(), 2 * 1024);
        assert_eq!(convert_human_size("2K").unwrap(), 2 * 1024);
        assert_eq!(convert_human_size("10m").unwrap(), 10 * 1024 * 1024);
        assert_eq!(convert_human_size("10M").unwrap(), 10 * 1024 * 1024);

        // allow space before units
        assert_eq!(convert_human_size("10 M").unwrap(), 10 * 1024 * 1024);

        // remove surrounding white space
        assert_eq!(convert_human_size(" 5 k ").unwrap(), 5 * 1024);

        // errors
        assert!(convert_human_size("k").is_err());
        assert!(convert_human_size("0").is_err());
        assert!(convert_human_size("1mb").is_err());
        assert!(convert_human_size(&i32::MAX.to_string()).is_err());
        assert!(convert_human_size(&(i32::MAX as u64 + 1).to_string()).is_err());
    }
}
