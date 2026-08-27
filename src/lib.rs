//! rescriptum — serving Proxmox VE answer files to the automated installer.
//!
//! The binary lives in `main.rs`; everything it is built from lives here, so the
//! behaviour can be tested directly rather than only through a socket. See CLAUDE.md
//! for the design constraints.

pub mod admin;
#[cfg(feature = "boot")]
pub mod boot;
pub mod capture;
pub mod cli;
pub mod config;
pub mod envfile;
pub mod facts;
pub mod format;
pub mod log;
pub mod merge;
pub mod select;
pub mod store;
