//! Thin shell crate built on top of `quickai-agent-sdk`.

pub use quickai_agent_sdk::{config, engine, runtime_bridge, tools};

pub mod agent;
pub mod native_runtime;
mod team;
