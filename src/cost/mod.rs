//! Token cost tracking and budget enforcement.
//!
//! This module provides types and infrastructure for recording LLM token usage
//! and enforcing configurable daily/monthly spending budgets.

pub mod tracker;
pub mod types;

pub use tracker::CostTracker;
pub use types::*;
