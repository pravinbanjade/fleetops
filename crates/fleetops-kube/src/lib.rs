//! Kubernetes environments on kube-rs: a client from a kubeconfig or
//! through an agent tunnel, and the `KubeOps` implementation over it.

pub mod apply;
pub mod client;
pub mod discovery;
pub mod env;
pub mod errors;
pub mod exec_auth;
mod generic;
pub mod helm;
pub mod kinds;
pub mod kubeconfig;
pub mod mask;
pub mod metrics;
pub mod printer;
pub mod process;
pub mod summaries;
pub mod templates;
pub mod tunnel;
pub mod watch;
pub mod workload_logs;

pub use client::KubeClient;
