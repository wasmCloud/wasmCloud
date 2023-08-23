use super::Timestamp;

use serde::{Deserialize, Serialize};

/// A portion of a file. The `isLast` field indicates whether this chunk
/// is the last in a stream. The `offset` field indicates the 0-based offset
/// from the start of the file for this chunk.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Chunk {
    #[serde(rename = "objectId")]
    pub object_id: String,
    #[serde(rename = "containerId")]
    pub container_id: String,
    /// bytes in this chunk
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub bytes: Vec<u8>,
    /// The byte offset within the object for this chunk
    #[serde(default)]
    pub offset: u64,
    /// true if this is the last chunk
    #[serde(rename = "isLast")]
    #[serde(default)]
    pub is_last: bool,
}

/// Response from actor after receiving a download chunk.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChunkResponse {
    /// If set and `true`, the sender will stop sending chunks,
    #[serde(rename = "cancelDownload")]
    #[serde(default)]
    pub cancel_download: bool,
}

/// Metadata for a container.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContainerMetadata {
    /// Container name
    #[serde(rename = "containerId")]
    pub container_id: String,
    /// Creation date, if available
    #[serde(rename = "createdAt")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
}

/// Combination of container id and object id
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContainerObject {
    #[serde(rename = "containerId")]
    pub container_id: String,
    #[serde(rename = "objectId")]
    pub object_id: String,
}

/// Parameter to GetObject
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetObjectRequest {
    /// object to download
    #[serde(rename = "objectId")]
    pub object_id: String,
    /// object's container
    #[serde(rename = "containerId")]
    pub container_id: String,
    /// Requested start of object to retrieve.
    /// The first byte is at offset 0. Range values are inclusive.
    /// If rangeStart is beyond the end of the file,
    /// an empty chunk will be returned with isLast == true
    #[serde(rename = "rangeStart")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_start: Option<u64>,
    /// Requested end of object to retrieve. Defaults to the object's size.
    /// It is not an error for rangeEnd to be greater than the object size.
    /// Range values are inclusive.
    #[serde(rename = "rangeEnd")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_end: Option<u64>,
}

/// Response to GetObject
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetObjectResponse {
    /// indication whether the request was successful
    #[serde(default)]
    pub success: bool,
    /// If success is false, this may contain an error
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The provider may begin the download by returning a first chunk
    #[serde(rename = "initialChunk")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_chunk: Option<Chunk>,
    /// Length of the content. (for multi-part downloads, this may not
    /// be the same as the length of the initial chunk)
    #[serde(rename = "contentLength")]
    #[serde(default)]
    pub content_length: u64,
    /// A standard MIME type describing the format of the object data.
    #[serde(rename = "contentType")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Specifies what content encodings have been applied to the object
    /// and thus what decoding mechanisms must be applied to obtain the media-type
    #[serde(rename = "contentEncoding")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
}

/// Result of input item
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ItemResult {
    #[serde(default)]
    pub key: String,
    /// whether the item succeeded or failed
    #[serde(default)]
    pub success: bool,
    /// optional error message for failures
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Parameter to list_objects.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListObjectsRequest {
    /// Name of the container to search
    #[serde(rename = "containerId")]
    #[serde(default)]
    pub container_id: String,
    /// Request object names starting with this value. (Optional)
    #[serde(rename = "startWith")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_with: Option<String>,
    /// Continuation token passed in ListObjectsResponse.
    /// If set, `startWith` is ignored. (Optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
    /// Last item to return (inclusive terminator) (Optional)
    #[serde(rename = "endWith")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_with: Option<String>,
    /// Optionally, stop returning items before returning this value.
    /// (exclusive terminator)
    /// If startFrom is "a" and endBefore is "b", and items are ordered
    /// alphabetically, then only items beginning with "a" would be returned.
    /// (Optional)
    #[serde(rename = "endBefore")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_before: Option<String>,
    /// maximum number of items to return. If not specified, provider
    /// will return an initial set of up to 1000 items. if maxItems > 1000,
    /// the provider implementation may return fewer items than requested.
    /// (Optional)
    #[serde(rename = "maxItems")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u32>,
}

/// Respose to list_objects.
/// If `isLast` is false, the list was truncated by the provider,
/// and the remainder of the objects can be requested with another
/// request using the `continuation` token.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListObjectsResponse {
    /// set of objects returned
    pub objects: Vec<ObjectMetadata>,
    /// Indicates if the item list is complete, or the last item
    /// in a multi-part response.
    #[serde(rename = "isLast")]
    #[serde(default)]
    pub is_last: bool,
    /// If `isLast` is false, this value can be used in the `continuation` field
    /// of a `ListObjectsRequest`.
    /// Clients should not attempt to interpret this field: it may or may not
    /// be a real key or object name, and may be obfuscated by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObjectMetadata {
    /// Object identifier that is unique within its container.
    /// Naming of objects is determined by the capability provider.
    /// An object id could be a path, hash of object contents, or some other unique identifier.
    #[serde(rename = "objectId")]
    pub object_id: String,
    /// container of the object
    #[serde(rename = "containerId")]
    pub container_id: String,
    /// size of the object in bytes
    #[serde(rename = "contentLength")]
    #[serde(default)]
    pub content_length: u64,
    /// date object was last modified
    #[serde(rename = "lastModified")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<Timestamp>,
    /// A MIME type of the object
    /// see http://www.w3.org/Protocols/rfc2616/rfc2616-sec14.html#sec14.17
    /// Provider implementations _may_ return None for this field for metadata
    /// returned from ListObjects
    #[serde(rename = "contentType")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Specifies what content encodings have been applied to the object
    /// and thus what decoding mechanisms must be applied to obtain the media-type
    /// referenced by the contentType field. For more information,
    /// see http://www.w3.org/Protocols/rfc2616/rfc2616-sec14.html#sec14.11.
    /// Provider implementations _may_ return None for this field for metadata
    /// returned from ListObjects
    #[serde(rename = "contentEncoding")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
}

/// Parameter to PutChunk operation
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PutChunkRequest {
    /// upload chunk from the file.
    /// if chunk.isLast is set, this will be the last chunk uploaded
    pub chunk: Chunk,
    /// This value should be set to the `streamId` returned from the initial PutObject.
    #[serde(rename = "streamId")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    /// If set, the receiving provider should cancel the upload process
    /// and remove the file.
    #[serde(rename = "cancelAndRemove")]
    #[serde(default)]
    pub cancel_and_remove: bool,
}

/// Parameter for PutObject operation
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PutObjectRequest {
    /// File path and initial data
    pub chunk: Chunk,
    /// A MIME type of the object
    /// see http://www.w3.org/Protocols/rfc2616/rfc2616-sec14.html#sec14.17
    #[serde(rename = "contentType")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Specifies what content encodings have been applied to the object
    /// and thus what decoding mechanisms must be applied to obtain the media-type
    /// referenced by the contentType field. For more information,
    /// see http://www.w3.org/Protocols/rfc2616/rfc2616-sec14.html#sec14.11.
    #[serde(rename = "contentEncoding")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
}

/// Response to PutObject operation
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PutObjectResponse {
    /// If this is a multipart upload, `streamId` must be returned
    /// with subsequent PutChunk requests
    #[serde(rename = "streamId")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
}

/// parameter to removeObjects
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoveObjectsRequest {
    /// name of container
    #[serde(rename = "containerId")]
    pub container_id: String,
    /// list of object names to be removed
    pub objects: Vec<String>,
}
