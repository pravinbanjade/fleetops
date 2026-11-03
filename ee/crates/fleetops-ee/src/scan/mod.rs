//! Image vulnerability scanning (vuln-scanning spec, Enterprise): Trivy runs
//! as a throwaway container on each Docker host, or as a Job on each
//! cluster, and the results feed a fleet-wide patch queue.

pub mod config;
pub mod model;
pub mod trivy;
