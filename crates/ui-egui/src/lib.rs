//! dbee user interface on egui (layer L6). Thin: panels read engine state and act through
//! [`DbeeApp::run`], which reaches the UI-only commands ([`ui_commands`]) and
//! `Session::execute`. The only crate besides the apps that may depend on egui/eframe.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod app;
pub mod automation;
pub mod components;
pub mod conn_form;
pub mod connections;
pub mod control;
pub mod control_server;
pub mod dialogs;
pub mod editors;
pub mod filter_bar;
pub mod grid;
pub mod menu;
pub mod paging;
pub mod picker;
pub mod query_tab;
pub mod quick_search;
pub(crate) mod quiet;
pub mod shortcuts;
pub mod structure_view;
pub mod table_tab;
pub mod theme;
pub mod tree;
pub mod ui_commands;
pub mod value_panel;
pub mod workspace;

pub use app::{Clipboard, DbeeApp};
pub use control::ControlRequest;
