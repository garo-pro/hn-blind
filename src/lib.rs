//! Core of hn-blind, an accessible Hacker News client.
//!
//! Split out from the binary so the parts worth testing — the API client, the
//! HTML-to-speech-text conversion, the wording of every announcement, and the
//! projection of application state into an AccessKit tree — can be exercised
//! without opening a window.

pub mod app;
pub mod config;
pub mod hn;
pub mod html;
pub mod menu;
pub mod preferences;
pub mod settings;
pub mod speech;
pub mod templates;
