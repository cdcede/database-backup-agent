//! Infrastructure layer — concrete implementations.
//!
//! This is the "Adapters" half of Hexagonal Architecture.
//! Each module implements a port trait or provides infrastructure services.

pub mod config_loader;
pub mod mssql;
pub mod compression;
pub mod storage;
pub mod telegram;
