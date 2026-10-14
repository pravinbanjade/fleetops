use crate::{
    auth::{limiter::LoginLimiter, setup::SetupGuard},
    envs::EnvRegistry,
    tasks::TaskManager,
    ws::hub::Hubs,
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
    /// First reconnect delay for dropped Docker event streams; doubles up
    /// to 30 s.
    pub ws_backoff: Duration,
    /// How often an open websocket re-checks its session.
    pub ws_session_check: Duration,
    /// How often the server pings each websocket. A client that sends
    /// nothing (not even a pong) for two intervals is disconnected.
    pub ws_ping_interval: Duration,
    /// Messages buffered per websocket subscription (spec default 1024).
    pub ws_queue_capacity: usize,
    /// How often the server pings each agent; two unanswered pings end the session.
    pub agent_ping_interval: Duration,
    /// How long a Kubernetes watch outlives its last subscriber (spec §2.4).
    pub kube_watch_grace: Duration,
    /// Which exec plugins kubeconfig users may run (spec 3k §1); off by default.
    pub kube_exec: fleetops_kube::exec_auth::ExecPolicy,
    /// Renders charts for install and upgrade (spec 3j §2); `None` turns them off.
    pub helm: Option<std::sync::Arc<dyn fleetops_kube::helm::render::ChartRenderer>>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            setup_window: Duration::from_secs(600),
            hsts: false,
            ws_backoff: Duration::from_secs(1),
            ws_session_check: Duration::from_secs(30),
            ws_ping_interval: Duration::from_secs(30),
            ws_queue_capacity: crate::ws::queue::QUEUE_CAPACITY,
            agent_ping_interval: Duration::from_secs(15),
            kube_watch_grace: Duration::from_secs(30),
            kube_exec: Default::default(),
            helm: None,
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
    pub hubs: Arc<Hubs>,
    pub tasks: Arc<TaskManager>,
    /// Live agent sessions, shared with the env factory.
    pub agents: Arc<fleetops_tunnel::registry::Registry>,
    /// When each environment's last `agent.disconnect` was audited, so a
    /// flapping agent writes one row per five minutes.
    pub agent_disconnects: Arc<std::sync::Mutex<std::collections::HashMap<i64, Instant>>>,
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
            hubs: Arc::new(Hubs::default()),
            tasks: Arc::new(TaskManager::default()),
            agents: Arc::default(),
            agent_disconnects: Arc::default(),
        }
    }

    /// Shares `agents` with the env factory, so agent environments see the
    /// sessions the connect handler registers.
    pub fn with_agents(mut self, agents: Arc<fleetops_tunnel::registry::Registry>) -> Self {
        self.agents = agents;
        self
    }
}
