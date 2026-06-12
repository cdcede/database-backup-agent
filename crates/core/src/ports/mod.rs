//! Ports layer — trait definitions (interfaces).
//!
//! Defines the contracts that infrastructure implementations must fulfill.
//! The domain and application layers depend on these traits, never on concrete types.
//!
//! This is the "Ports" half of Ports & Adapters (Hexagonal Architecture).

pub mod database;
pub mod compression;
pub mod storage;
pub mod notifier;
