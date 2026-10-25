//! Storage adapters — implementations of the `Storage` port.
//!
//! Provides concrete storage implementations (e.g. Local Directory, AWS S3).

pub mod google_drive;
pub mod local;
pub mod mirrored;

#[cfg(feature = "s3")]
pub mod s3;
