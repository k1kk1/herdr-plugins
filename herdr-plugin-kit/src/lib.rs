//! Shared foundation for the Herdr plugins in this repository.
//!
//! Four plugins — Pane Manager, Layout Tools, Navigator and Command Palette —
//! all need the same things: a Herdr socket client, the rule for turning a
//! pane into something a human recognises, a keyboard picker, and a way to
//! find out what the user was looking at. Sharing them keeps the plugins
//! consistent with each other, which is the point of the whole set: the same
//! pane looks the same and is named the same wherever it appears.

pub mod config;
pub mod context;
pub mod herdr;
pub mod label;
pub mod layout;
pub mod ui;

pub use anyhow::{anyhow, bail, Context, Result};

/// What an operation did, phrased for a toast.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub message: String,
    pub detail: Option<String>,
}

impl Outcome {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Report the result as a toast, and on stdout for `herdr plugin log`.
    pub fn report(&self, herdr: &herdr::Herdr) {
        println!("{}", self.message);
        if let Some(detail) = &self.detail {
            println!("{detail}");
        }
        herdr.notify(&self.message, self.detail.as_deref());
    }
}
