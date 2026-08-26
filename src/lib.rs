//! Core of hn-blind, an accessible Hacker News client.
//!
//! Split out from the binary so the parts worth testing — the API client, the
//! HTML-to-speech-text conversion, the wording of every announcement, and the
//! state that decides which of them applies — can be exercised without
//! opening a window. Nothing here links against wxWidgets; `main.rs` owns
//! every widget.

pub mod app;
pub mod config;
pub mod hn;
pub mod html;
pub mod menu;
pub mod preferences;
pub mod settings;
pub mod speech;
pub mod templates;
