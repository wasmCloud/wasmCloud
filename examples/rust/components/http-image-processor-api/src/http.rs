use bytes::{Buf, Bytes};
use image::ImageFormat;

/// Parse an [`ImageFormat`] from a string that vaguely looks like a MIME content type
pub(crate) fn fuzzy_parse_image_format_from_mime(s: &str) -> Option<ImageFormat> {
    if s.contains("image/jpeg") {
        return Some(ImageFormat::Jpeg);
    }
    if s.contains("image/jpg") {
        return Some(ImageFormat::Jpeg);
    }
    if s.contains("image/tiff") {
        return Some(ImageFormat::Tiff);
    }
    if s.contains("image/webp") {
        return Some(ImageFormat::WebP);
    }
    if s.contains("image/gif") {
        return Some(ImageFormat::Gif);
    }
    if s.contains("image/bmp") {
        return Some(ImageFormat::Bmp);
    }
    None
}

/// Things that can be built from headers
pub(crate) trait FromHttpHeader
where
    Self: Sized,
{
    /// Build a value of the implementing type from header
    fn from_header(k: &str, values: &Vec<String>) -> Option<Self>;
}

impl FromHttpHeader for ImageFormat {
    fn from_header(k: &str, values: &Vec<String>) -> Option<Self> {
        if k.to_lowercase() != "content-type" {
            return None;
        }

        values
            .iter()
            .map(String::as_str)
            .map(str::to_lowercase)
            .find_map(|s| fuzzy_parse_image_format_from_mime(&s))
    }
}

/// Things that can be built to headers
pub(crate) trait ToHttpHeader
where
    Self: Sized,
{
    /// Build a value of the implementing type to header
    fn to_header(&self) -> (&str, &str);
}

impl ToHttpHeader for ImageFormat {
    fn to_header(&self) -> (&str, &str) {
        (
            "Content-Type",
            match &self {
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::Tiff => "image/tiff",
                ImageFormat::WebP => "image/webp",
                ImageFormat::Gif => "image/gif",
                ImageFormat::Bmp => "image/Bmp",
                _ => "application/octet-stream",
            },
        )
    }
}

/// Wrapper used to implement multipart
pub(crate) struct RequestBodyBytes<'a> {
    pub content_type: &'a str,
    pub body: Bytes,
}

impl multipart::server::HttpRequest for RequestBodyBytes<'_> {
    type Body = bytes::buf::Reader<Bytes>;

    fn multipart_boundary(&self) -> Option<&str> {
        self.content_type.find("boundary=").map(|idx| {
            let start = idx + "boundary=".len();
            let end = self.content_type[idx..]
                .find(";")
                .unwrap_or(self.content_type.len());
            return &self.content_type[start..end];
        })
    }

    fn body(self) -> bytes::buf::Reader<Bytes> {
        self.body.clone().reader()
    }
}
