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
/// Out-of-band controllers: where a machine's BMC, PiKVM or PDU is described.
pub mod controllers;
/// Editing an answer document in the operator's own editor, through the guard.
pub mod edit;
pub mod envfile;
pub mod facts;
pub mod format;
/// A write that cannot leave the answer set broken — the rule, without the HTTP.
pub mod guard;
/// A machine reporting that it finished installing, and the claim being dropped.
pub mod installed;
pub mod log;
pub mod merge;
/// Driving a controller: what `power on`, `off`, `pxe` and `status` actually do.
pub mod power;
/// Talking to a Redfish service, through `curl` — there is no TLS in this binary.
pub mod redfish;
pub mod select;
pub mod store;
/// Following the server's log from another process: rotation, bounded buffer, filters.
pub mod tail;
pub mod tomlconfig;
/// What a terminal interface keeps, and when it may do work. No drawing, on purpose.
pub mod tui;
