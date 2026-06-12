//! Application layer — use cases and orchestration.
//!
//! Coordinates domain entities and port traits to implement business workflows.
//! This layer knows WHAT to do but not HOW (that's infrastructure's job).
//!
pub mod backup_service;
pub mod retention;
