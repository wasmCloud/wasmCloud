use std::io::{Cursor, Read};
use std::str::FromStr;

use anyhow::{anyhow, bail, ensure, Context as _, Result};
use bytes::{Bytes, BytesMut};
use image::{DynamicImage, ImageFormat, ImageReader};
use multipart::server::MultipartField;
use serde::Deserialize;
use url::{Host, Url};

use crate::wasi::http::outgoing_handler;
use crate::wasi::http::outgoing_handler::OutgoingRequest;
use crate::wasi::http::types::{Fields, IncomingBody, IncomingRequest, Scheme};
use crate::wasi::io::streams::StreamError;
use crate::wasi::logging::logging::{log, Level};

use crate::http::{fuzzy_parse_image_format_from_mime, FromHttpHeader, RequestBodyBytes};
use crate::objstore::read_object;
use crate::{extract_headers, LOG_CONTEXT, MAX_READ_BYTES};

/// Image that can be used by default if no image is provided (wasmcloud logo)
const DEFAULT_IMAGE_BYTES: &[u8] = std::include_bytes!("../wasmcloud-logo.png");

/// wasmCloud Link name
type LinkName = String;

/// S3 Bucket
type Bucket = String;

/// Key inside an S3 bucket
type Key = String;

/// Source of an image
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "type")]
pub(crate) enum ImagePath {
    /// Use the default image for this component
    #[default]
    DefaultImage,
    /// Indicates bytes that were attached (normally to a [`ImageProcessingRequest`])
    Attached,
    /// Remote image stored at a given URL
    RemoteHttps { url: Url },
    /// A previously uploaded file will have a Url that is specific to S3
    /// which we have to retrieve
    ///
    /// We expect this to come in as urls like `s3://<bucket>/<key>?link_name=<link name>`
    Blobstore { path: BlobstorePath },
}

/// Information necessary to direct an upload or download from a blobstore
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct BlobstorePath {
    pub(crate) link_name: LinkName,
    pub(crate) bucket: Bucket,
    pub(crate) key: Key,
}

impl FromStr for BlobstorePath {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Url::parse(s)
            .context("failed to parse URL (s3 scheme) while parsing BlobstorePath: {e}")?
            .try_into()
    }
}

impl TryFrom<Url> for BlobstorePath {
    type Error = anyhow::Error;

    fn try_from(url: Url) -> Result<Self> {
        let bucket = match url.host().context("failed to get host")? {
            Host::Domain(s) => s.to_string(),
            _ => bail!("invalid host version it must be in the DNS domain-name style (ex. 's3://bucket/key')"),
        };
        let key = url.path().to_string();
        let link_name = url
            .query_pairs()
            .find_map(|v| {
                if v.0 == "link_name" {
                    Some(v.1.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "default".into());

        Ok(Self {
            link_name,
            bucket,
            key,
        })
    }
}

impl FromStr for ImagePath {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "default" => Ok(Self::DefaultImage),
            "attached" => Ok(Self::Attached),
            s => {
                let url = Url::parse(s)
                    .map_err(|e| anyhow!("failed to parse URL while building ImagePath: {e}"))?;
                let scheme = url.scheme();
                match scheme {
                    "s3" => {
                        let bucket = match url.host().context("failed to get host")? {
                            Host::Domain(s) => s.to_string(),
                            _ => bail!("invalid host version it must be in the DNS domain-name style (ex. 's3://bucket/key')"),
                        };
                        let key = url.path().to_string();
                        let link_name = url
                            .query_pairs()
                            .find_map(|v| {
                                if v.0 == "link_name" {
                                    Some(v.1.to_string())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| "default".into());
                        Ok(Self::Blobstore {
                            path: BlobstorePath {
                                link_name,
                                bucket,
                                key,
                            },
                        })
                    }
                    "https" => Ok(Self::RemoteHttps { url }),
                    _ => bail!("url scheme [{scheme}] is not supported"),
                }
            }
        }
    }
}

/// Operations that can be done on the image
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "type")]
pub(crate) enum ImageOperation {
    /// Perform no changes on the image
    #[default]
    NoOp,
    /// Run a grayscale filter on the image
    Grayscale,
    /// Resize the image with
    Resize { height_px: u32, width_px: u32 },
}

impl FromStr for ImageOperation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "no-op" | "filter:no-op" => Ok(Self::NoOp),
            "filter:grayscale" => Ok(Self::Grayscale),
            segment if segment.starts_with("resize:") => {
                let segment_chars = segment.chars();
                let width_px_str = segment_chars
                    .clone()
                    .skip("resize:".len())
                    .take_while(|c| *c != 'x')
                    .collect::<String>();
                let width_px = width_px_str
                    .as_str()
                    .parse::<u32>()
                    .context("failed to parse resize width pixels (first number)")?;
                let height_px = segment_chars
                    .skip("resize:".len() + width_px_str.len() + 1)
                    .collect::<String>()
                    .as_str()
                    .parse::<u32>()
                    .context("failed to parse resize width pixels (first number)")?;
                Ok(Self::Resize {
                    width_px,
                    height_px,
                })
            }
            _ => bail!("unsupported ImageOperation [{s}]"),
        }
    }
}

/// Image processing
#[derive(Debug, Deserialize)]
pub(crate) struct ImageProcessingRequest {
    /// Source of the image
    pub(crate) image_source: ImagePath,
    /// Format of the image
    #[serde(deserialize_with = "deserialize_image_format_opt")]
    pub(crate) image_format: Option<ImageFormat>,
    /// Operations to perform on the image
    pub(crate) operations: Vec<ImageOperation>,
    /// If specified, where to upload the original image to S3
    pub(crate) blobstore_upload_original: Option<BlobstorePath>,
    /// Whether to upload the transformed image to S3
    pub(crate) blobstore_upload_output: Option<BlobstorePath>,
    /// Whether to perform AI analysis on the incoming image rather than performing any operations
    ///
    /// This is a separate option (and if specified operations should be ignored) because the response
    /// is dramatically different from just the returned image
    #[serde(default)]
    #[allow(unused)]
    pub(crate) analyze_with_ai: bool,
    /// Image data
    ///
    /// This *may* be empty in the case that the image source is remote and has not been fetched yet
    #[serde(default)]
    image_data: Bytes,
}

impl ImageProcessingRequest {
    /// Build an [`ImageProcessingRequest`] from a WASI HTTP `IncomingRequest` (normally generated from bindgen)
    pub(crate) fn from_incoming_request(request: IncomingRequest) -> Result<Self> {
        if request.is_multipart_formdata() {
            Self::from_multipart_formdata(request)
        } else if request.is_json() {
            Self::from_json(request)
        } else {
            Self::from_urlencoded(request)
        }
    }

    /// Build an [`ImageProcessingRequest`] from a WASI HTTP `IncomingRequest` (normally generated from bindgen)
    /// that is known to be urlencoded (i.e. all relevant information is in the URL)
    fn from_urlencoded(request: IncomingRequest) -> Result<Self> {
        let path_with_query = request.path_with_query();
        let (path, query) = path_with_query
            .as_deref()
            .unwrap_or("/")
            .split_once('?')
            .unwrap_or(("/", ""));
        let query_params = query
            .split('&')
            .filter_map(|v| v.split_once('='))
            .collect::<Vec<(&str, &str)>>();
        let headers = extract_headers(&request);

        let image_format = headers
            .iter()
            .find(|(k, _v)| k.to_lowercase() == "content-type")
            .and_then(|(k, vs)| ImageFormat::from_header(k, vs));

        let mut operations = Vec::new();
        let mut path_segment = path;
        let mut image_source: Option<ImagePath> = None;

        // Parse segments
        while let Some((lhs, rhs)) = path_segment.split_once('/') {
            if lhs.is_empty() {
                path_segment = rhs;
                continue;
            }
            if rhs.is_empty() {
                break;
            }

            match lhs {
                "process" => {
                    continue;
                }
                "noop" => {
                    operations.push(ImageOperation::NoOp);
                }
                "filter:grayscale" => operations.push(ImageOperation::Grayscale),
                segment if segment.starts_with("resize:") => {
                    match ImageOperation::from_str(segment) {
                        Ok(v) => operations.push(v),
                        Err(e) => {
                            log(
                                Level::Warn,
                                LOG_CONTEXT,
                                &format!("failed to parse image resize operation: {e}"),
                            );
                        }
                    }
                }
                // Anything else we can attempt to parse as image source
                s => {
                    image_source = match ImagePath::from_str(s) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            log(
                                Level::Warn,
                                LOG_CONTEXT,
                                &format!("failed to image path: {e}"),
                            );
                            None
                        }
                    };
                }
            }
            path_segment = rhs;
        }

        let mut blobstore_upload_original = None;
        let mut blobstore_upload_output = None;
        let mut analyze_with_ai = false;
        for (k, v) in query_params {
            match k.to_lowercase().as_str() {
                "blobstore_upload_original" => match BlobstorePath::from_str(v) {
                    Ok(path) => {
                        blobstore_upload_original = Some(path);
                    }
                    Err(e) => {
                        log(
                            Level::Warn,
                            LOG_CONTEXT,
                            &format!("failed to parse blobstore_upload_original value: {e}"),
                        );
                    }
                },
                "blobstore_upload_output" => match BlobstorePath::from_str(v) {
                    Ok(path) => {
                        blobstore_upload_output = Some(path);
                    }
                    Err(e) => {
                        log(
                            Level::Warn,
                            LOG_CONTEXT,
                            &format!("failed to parse blobstore_upload_output value: {e}"),
                        );
                    }
                },
                "analyze_with_ai" => {
                    analyze_with_ai = true;
                }
                _ => continue,
            }
        }

        Ok(Self {
            image_source: image_source.context("failed to find parse source")?,
            image_format,
            operations,
            blobstore_upload_original,
            blobstore_upload_output,
            analyze_with_ai,
            image_data: Bytes::new(),
        })
    }

    /// Build an [`ImageProcessingRequest`] from a WASI HTTP `IncomingRequest` (normally generated from bindgen)
    /// that is known to be `multipart/form-data` encoded
    fn from_multipart_formdata(request: IncomingRequest) -> Result<Self> {
        let headers = extract_headers(&request);
        let mut multipart_body = multipart::server::Multipart::from_request(RequestBodyBytes {
            content_type: headers
                .iter()
                .find(|(k, _v)| k.to_lowercase() == "content-type")
                .map(|(_k, v)| v.join(";"))
                .as_deref()
                .unwrap_or_default(),
            body: request.read_body()?,
        })
        .map_err(|_| anyhow!("failed to read multipart body"))?;

        let mut image_source = None;
        let mut image_format = None;
        let mut operations = Vec::new();
        let mut blobstore_upload_original = None;
        let mut blobstore_upload_output = None;
        let mut analyze_with_ai = false;
        let mut image_data = None;
        while let Ok(Some(MultipartField { headers, mut data })) = multipart_body.read_entry() {
            let mut value = Vec::new();
            data.read_to_end(&mut value).with_context(|| {
                format!("failed to read formdata content for [{}]", headers.name)
            })?;
            match headers.name.as_ref() {
                "image_source" => match ImagePath::from_str(&String::from_utf8(value)?) {
                    Ok(ip) => {
                        image_source = Some(ip);
                    }
                    Err(e) => {
                        log(
                            Level::Warn,
                            LOG_CONTEXT,
                            &format!("failed to parse image source: {e}"),
                        );
                    }
                },
                "image_format" => {
                    match fuzzy_parse_image_format_from_mime(&String::from_utf8(value.clone())?) {
                        Some(f) => {
                            image_format = Some(f);
                        }
                        None => {
                            log(
                                Level::Warn,
                                LOG_CONTEXT,
                                &format!(
                                    "failed to parse image format value [{}] (it should look like a MIME type)", 
                                    String::from_utf8_lossy(&value),
                                ),
                            );
                        }
                    }
                }
                "operations[]" => match ImageOperation::from_str(&String::from_utf8(value)?) {
                    Ok(op) => operations.push(op),
                    Err(e) => {
                        log(
                            Level::Warn,
                            LOG_CONTEXT,
                            &format!("failed to parse operation value: {e}"),
                        );
                    }
                },
                "blobstore_upload_original" => {
                    match BlobstorePath::from_str(&String::from_utf8(value)?) {
                        Ok(bp) => {
                            blobstore_upload_original = Some(bp);
                        }
                        Err(e) => {
                            log(
                                Level::Warn,
                                LOG_CONTEXT,
                                &format!("failed to parse blobstore_upload_original value: {e}"),
                            );
                        }
                    }
                }
                "blobstore_upload_output" => {
                    match BlobstorePath::from_str(&String::from_utf8(value)?) {
                        Ok(bp) => {
                            blobstore_upload_output = Some(bp);
                        }
                        Err(e) => {
                            log(
                                Level::Warn,
                                LOG_CONTEXT,
                                &format!("failed to parse blobstore_upload_output value: {e}"),
                            );
                        }
                    }
                }
                "analyze_with_ai" => {
                    analyze_with_ai = true;
                }
                "image" => {
                    image_data = Some(Bytes::from(value));
                }
                _ => continue,
            }
        }

        if image_data.is_none() && image_source.is_none() {
            bail!("either image_source or image (a file) must be supplied");
        }

        Ok(Self {
            image_source: image_source.context("failed to parse image source")?,
            image_format,
            operations,
            blobstore_upload_original,
            blobstore_upload_output,
            analyze_with_ai,
            image_data: image_data.unwrap_or_default(),
        })
    }

    /// Build an [`ImageProcessingRequest`] from a WASI HTTP `IncomingRequest` (normally generated from bindgen)
    /// that is known to be `multipart/form-data` encoded
    fn from_json(request: IncomingRequest) -> Result<Self> {
        let body_bytes = request.read_body()?;
        serde_json::from_slice(&body_bytes).context("failed to read request body")
    }

    /// Fetch the bytes that make up the image
    pub(crate) fn fetch_image(&self) -> Result<Bytes> {
        match &self.image_source {
            ImagePath::DefaultImage => Ok(Bytes::from(DEFAULT_IMAGE_BYTES)),
            ImagePath::Attached => Ok(self.image_data.clone()),
            ImagePath::RemoteHttps { url } => {
                let req = OutgoingRequest::new(Fields::new());
                req.set_scheme(Some(&Scheme::Https))
                    .map_err(|()| anyhow!("failed to set scheme"))?;
                req.set_authority(Some(url.authority()))
                    .map_err(|()| anyhow!("failed to set authority"))?;
                req.set_path_with_query(Some(url.path()))
                    .map_err(|()| anyhow!("failed to set path and query"))?;
                req.fetch_bytes()
            }
            ImagePath::Blobstore {
                path:
                    BlobstorePath {
                        link_name,
                        bucket,
                        key,
                    },
            } => read_object(link_name, bucket, key),
        }
    }
}

impl OutgoingRequest {
    fn fetch_bytes(self) -> Result<Bytes> {
        let resp =
            outgoing_handler::handle(self, None).map_err(|e| anyhow!("request failed: {e}"))?;
        resp.subscribe().block();
        let response = resp
            .get()
            .context("HTTP request response missing")?
            .map_err(|()| anyhow!("HTTP request response requested more than once"))?
            .map_err(|code| anyhow!("HTTP request failed (error code {code})"))?;

        if response.status() != 200 {
            bail!("response failed, status code [{}]", response.status());
        }

        let response_body = response
            .consume()
            .map_err(|()| anyhow!("failed to get incoming request body"))?;

        let mut buf = BytesMut::with_capacity(MAX_READ_BYTES);
        let stream = response_body
            .stream()
            .expect("failed to get HTTP request response stream");
        loop {
            match stream.read(MAX_READ_BYTES as u64) {
                Ok(bytes) if bytes.is_empty() => break,
                Ok(bytes) => {
                    ensure!(
                        bytes.len() <= MAX_READ_BYTES,
                        "read more bytes than requested"
                    );
                    buf.extend(bytes);
                }
                Err(StreamError::Closed) => break,
                Err(e) => bail!("failed to read bytes: {e}"),
            }
        }
        let _ = IncomingBody::finish(response_body);

        Ok(buf.freeze())
    }
}

/// Transform the bytes of a given image
pub(crate) fn transform_image(
    content_type: Option<ImageFormat>,
    image_bytes: Bytes,
    operations: Vec<ImageOperation>,
) -> Result<DynamicImage> {
    let cursor = Cursor::new(image_bytes);
    let reader = if let Some(ct) = content_type {
        ImageReader::with_format(cursor, ct)
    } else {
        ImageReader::new(cursor)
            .with_guessed_format()
            .map_err(|e| anyhow!("failed to guess format: {e}"))?
    };
    let mut image = reader
        .decode()
        .map_err(|e| anyhow!("failed to decode image: {e}"))?;

    for op in operations {
        log(
            Level::Info,
            LOG_CONTEXT,
            format!("performing operation [{op:?}]").as_str(),
        );
        match op {
            ImageOperation::NoOp => {
                continue;
            }
            ImageOperation::Grayscale => {
                image = image.grayscale();
            }
            ImageOperation::Resize {
                height_px,
                width_px,
            } => {
                image = image.resize(width_px, height_px, image::imageops::FilterType::Nearest);
            }
        }
    }

    Ok(image)
}

pub(crate) fn deserialize_image_format_opt<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ImageFormat>, D::Error> {
    use serde::de::Error;
    let s = Option::<String>::deserialize(deserializer)?;
    match s.as_ref().map(String::as_str) {
        None => Ok(None),
        Some("image/jpeg") => Ok(Some(ImageFormat::Jpeg)),
        Some("image/jpg") => Ok(Some(ImageFormat::Jpeg)),
        Some("image/tiff") => Ok(Some(ImageFormat::Tiff)),
        Some("image/webp") => Ok(Some(ImageFormat::WebP)),
        Some("image/gif") => Ok(Some(ImageFormat::Gif)),
        Some("image/bmp") => Ok(Some(ImageFormat::Bmp)),
        _ => Err(D::Error::custom(format!(
            "unrecognized image format (use content MIME types like 'image/jpeg')"
        ))),
    }
}
