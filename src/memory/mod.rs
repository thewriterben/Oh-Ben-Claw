//! Oh-Ben-Claw memory subsystem.
//!
//! The memory subsystem provides persistent storage for the agent's
//! conversation history, learned facts, and other information.
//!
//! # Backends
//!
//! | Backend    | Status  | Description                                    |
//! |------------|---------|------------------------------------------------|
//! | SQLite     | Planned | Embedded relational database (default)         |
//! | Markdown   | Planned | Human-readable flat files                      |
//! | Vector     | Planned | Semantic search via embeddings                 |
//! | None       | Planned | In-memory only (no persistence)                |
