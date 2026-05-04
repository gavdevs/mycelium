//! mycel-lsp — Phase 1 ships only the multilspy-backed resolver.
//!
//! A `Resolver` trait abstraction is intentionally NOT defined here. Phase 1
//! has exactly one resolver implementation; introducing a trait for one impl
//! is premature. When a second resolver lands (native Rust LSP client, or a
//! mock for testing without Python), extract the trait at that point. Until
//! then, callers depend on `MultilspyResolver` directly.

pub mod multilspy;
pub mod protocol;

pub use multilspy::MultilspyResolver;
