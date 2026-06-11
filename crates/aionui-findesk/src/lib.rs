//! FinDesk-specific backend extensions (FinSAFE P0 spawn wrapping).

mod config;
mod register;

pub mod finsafe;

pub use config::FindeskConfig;
pub use register::{register_from_env, sync_finsafe_enabled_from_settings, FindeskHandles};
