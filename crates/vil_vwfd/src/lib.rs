//! # vil_vwfd — VIL VWFD Runtime
//!
//! Compile and execute VWFD workflows on VIL infrastructure.
//! Same VWFD YAML format as VFlow — different runtime, own compiler.

pub mod app;
pub mod audit;
pub mod cli;
pub mod compiler;
pub mod durability;
pub mod eval_bridge;
pub mod executor;
pub mod graph;
pub mod handler;
pub mod handler_provision;
pub mod loader;
pub mod manifests;
pub mod mcp;
pub mod plugin_loader;
pub mod process;
pub mod provision;
pub mod provision_admin;
pub mod registry;
pub mod saga;
pub mod spec;
pub mod spv1;
pub mod triggers;

pub use app::prelude;
pub use app::{app, NativeRegistry, VwfdApp};
pub use compiler::compile;
pub use durability::{DurabilityStore, StateStore};
pub use executor::{execute, ExecConfig, ExecError, ExecResult};
pub use graph::{NodeKind, VilwGraph};
pub use handler::WorkflowRouter;
pub use loader::{load_dir, load_yaml};
pub use provision::WorkflowRegistry;
pub use saga::{collect_compensations, run_compensation};
