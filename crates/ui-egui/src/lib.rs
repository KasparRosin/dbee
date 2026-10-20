//! dbee user interface on egui (layer L6). Thin: panels read engine state and act through
//! [`DbeeApp::run`], which reaches the UI-only commands ([`ui_commands`]) and
//! `Session::execute`. The only crate besides the apps that may depend on egui/eframe.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod app;
pub mod automation;
pub mod control;
pub mod control_server;
pub mod icons;
pub mod shortcuts;
pub mod theme;
pub mod ui_commands;

pub use app::DbeeApp;
pub use control::ControlRequest;
