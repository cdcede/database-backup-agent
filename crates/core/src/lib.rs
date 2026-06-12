//! # Backup Agent Core
//!
//! Shared library containing all business logic for the Backup Agent.
//! Used by both the GUI and the Windows Service binaries.
//!
//! ## Architecture (Clean Architecture)
//!
//! ```text
//! domain/          → Entities, value objects, error types (no external deps)
//! ports/           → Trait definitions (interfaces for the outside world)
//! application/     → Use cases, orchestration logic
//! infrastructure/  → Concrete implementations (SQL Server, S3, Telegram, etc.)
//! ipc/             → Inter-process communication protocol definitions
//! ```
//!
//! Dependencies always point inward: infrastructure → application → domain.
//! The domain layer has ZERO knowledge of infrastructure details.

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod ipc;
pub mod ports;
