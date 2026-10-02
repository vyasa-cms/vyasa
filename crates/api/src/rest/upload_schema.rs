//! OpenAPI shapes for request bodies that are files, so generated clients
//! and the API reference show a file field rather than a bare string.
//! Only the documentation uses them; the handlers read multipart fields.

#![allow(dead_code)]

/// A single file in the multipart field `file`.
#[derive(utoipa::ToSchema)]
pub struct FileUpload {
    /// The file.
    #[schema(value_type = String, format = Binary)]
    pub file: String,
}

/// A media upload: the file, plus optional text stored with it.
#[derive(utoipa::ToSchema)]
pub struct MediaUpload {
    /// The file.
    #[schema(value_type = String, format = Binary)]
    pub file: String,
    /// Alternative text for images.
    pub alt: Option<String>,
    /// Caption shown under the item.
    pub caption: Option<String>,
}

/// The raw bytes of one file.
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct RawBytes(pub Vec<u8>);
