//! Media domain: storage abstraction and upload service.

pub mod derivatives;
pub mod service;
pub mod storage;

pub use derivatives::{
    blurhash_for, delete_derivatives, derivative_paths, dimensions_of, generate_derivatives, SIZES,
};
pub use service::{
    check_upload, sanitize_file_name, sha256_hex, CropBox, ImageEdit, MediaService, MAX_BYTES,
};
pub use storage::{assert_backend_contract, LocalFsBackend, StorageBackend};
