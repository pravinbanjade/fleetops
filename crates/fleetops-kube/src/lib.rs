//! Kubernetes environments on kube-rs: a client from a kubeconfig or
//! through an agent tunnel, and the `KubeOps` implementation over it.

pub mod apply;
pub mod client;
pub mod env;
pub mod errors;
pub mod kinds;
pub mod kubeconfig;
pub mod mask;
pub mod summaries;
pub mod templates;
pub mod tunnel;
pub mod watch;

pub use client::KubeClient;
