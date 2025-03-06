#![warn(
    clippy::all,
    clippy::pedantic,
    clippy::cargo,
    clippy::nursery,
    clippy::unwrap_used
)]
#![allow(clippy::module_name_repetitions)]

// Library modules (from wash-lib)
pub mod lib {
    pub mod app;
    pub mod build;
    pub mod capture;
    pub mod cli;
    pub mod common;
    pub mod component;
    pub mod config;
    pub mod context;
    pub mod deps;
    pub mod drain;
    pub mod generate;
    pub mod id;
    pub mod keys;
    pub mod parser;
    pub mod plugin;
    pub mod registry;
    pub mod spier;
    pub mod start;
    pub mod wait;
}

// CLI modules
pub mod cli;

// Re-exports for backward compatibility
pub use cli::*;
