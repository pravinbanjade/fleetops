//! Notifications (notifications spec, Enterprise): rules match new CVEs from
//! the scanner and send them to Slack, email and webhooks.

pub mod db;
pub mod format;
pub mod matcher;
pub mod model;
pub mod notifier;
pub mod routes;
pub mod send;
