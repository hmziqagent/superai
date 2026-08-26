//! Harness adapters — concrete implementations.

pub mod aider;
pub mod claude_code;
pub mod codex_cli;

pub use aider::AiderAdapter;
pub use claude_code::ClaudeCodeAdapter;
pub use codex_cli::CodexCliAdapter;
