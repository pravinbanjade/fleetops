use crate::{
    auth::{limiter::LoginLimiter, setup::SetupGuard},
    envs::EnvRegistry,
};
use fleetops_core::env::EnvFactory;
use fleetops_store::Store;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub const LOGIN_MAX_FAILURES: u32 = 5;
pub const LOGIN_WINDOW: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// How long after process start first-run setup stays open.
    pub setup_window: Duration,
    /// Send HSTS on TLS responses. Off by default because the HTTP listener
    /// is always on and HSTS would break it for browsers.
    pub hsts: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            setup_window: Duration::from_secs(600),
            hsts: false,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub config: Arc<ApiConfig>,
    pub limiter: Arc<LoginLimiter>,
    pub setup: Arc<SetupGuard>,
    pub envs: Arc<EnvRegistry>,
}

impl AppState {
    /// Call `envs.load(&store)` afterwards to build stored environments.
    pub fn new(store: Store, config: ApiConfig, factory: Arc<dyn EnvFactory>) -> Self {
        let setup = SetupGuard::new(Instant::now(), config.setup_window);
        Self {
            store,
            config: Arc::new(config),
            limiter: Arc::new(LoginLimiter::new(LOGIN_MAX_FAILURES, LOGIN_WINDOW)),
            setup: Arc::new(setup),
            envs: Arc::new(EnvRegistry::new(factory)),
        }
    }
}
