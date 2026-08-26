//! Harness adapters — concrete implementations.

pub mod aider;
pub mod claude_code;
pub mod cline;
pub mod codex_cli;
pub mod opencode;

pub use aider::AiderAdapter;
pub use claude_code::ClaudeCodeAdapter;
pub use cline::ClineAdapter;
pub use codex_cli::CodexCliAdapter;
pub use opencode::OpenCodeAdapter;
