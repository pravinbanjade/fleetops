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

/// Builds Swarm cluster calls that travel over one agent session.
pub type SwarmThrough = Arc<
    dyn Fn(
            fleetops_tunnel::Session,
        ) -> fleetops_core::Result<Arc<dyn fleetops_core::swarm::SwarmOps>>
        + Send
        + Sync,
>;

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
    /// Where browsers reach this server (`FLEETOPS_PUBLIC_URL`), without a
    /// trailing slash; sign-in redirects need it.
    pub public_url: Option<String>,
    /// The data directory (`FLEETOPS_DATA`); backups and restores are
    /// prepared in it. `None` turns them off.
    pub data_dir: Option<std::path::PathBuf>,
    /// Stack deploy waits (stacks spec §4); shortened in tests.
    pub stack_timing: crate::stacks::Timing,
    /// Accept `http://` git URLs; only tests set it.
    pub git_allow_http: bool,
    /// Caps on one git fetch (stacks spec §8).
    pub git_limits: fleetops_git::Limits,
    /// The certificate FleetOps generated for its HTTPS listener (PEM), when
    /// it serves one: Swarm agents inside the swarm trust it (swarm spec §3).
    pub agent_ca_pem: Option<String>,
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
            public_url: None,
            data_dir: None,
            stack_timing: Default::default(),
            git_allow_http: false,
            git_limits: Default::default(),
            agent_ca_pem: None,
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
    /// Keyed by environment and Swarm node id (`""` for other agents).
    pub agent_disconnects: Arc<std::sync::Mutex<std::collections::HashMap<(i64, String), Instant>>>,
    /// Swarm calls through one agent session: the handshake checks a node
    /// before it is registered. The server supplies fleetops-docker's; `None`
    /// refuses Swarm agents.
    pub swarm_through: Option<SwarmThrough>,
    /// The edition and license in effect; Community unless an Enterprise
    /// build replaces it (editions spec §3.1).
    pub entitlements: Arc<dyn fleetops_core::edition::Entitlements>,
    /// Grants beyond direct bindings; none unless an Enterprise build adds
    /// team grants. Read only through `authz::effective_grant(s)`.
    pub extra_grants: Arc<dyn fleetops_core::access::ExtraGrants>,
    /// Custom-role grants made to users; none in Community.
    pub custom_roles: Arc<dyn fleetops_core::access::CustomRoles>,
    /// Single sign-on; local passwords only in Community.
    pub external_auth: Arc<dyn crate::external::ExternalAuth>,
    /// Enterprise sealed columns and migrations for backups; none in Community.
    pub backup_hooks: Arc<dyn crate::backup::BackupHooks>,
    /// Starts the server again after a restore is left pending.
    pub restart: Arc<dyn crate::backup::Restart>,
    /// Uploaded backups waiting for their restore.
    pub staged: Arc<crate::backup::StagedBackups>,
    /// Stacks with a deploy running, by stack id (one at a time per stack).
    pub stack_locks: Arc<std::sync::Mutex<std::collections::HashSet<i64>>>,
    /// Stacks with a git sync running, and whether another was asked for
    /// meanwhile.
    pub git_syncs: Arc<std::sync::Mutex<std::collections::HashMap<i64, bool>>>,
    /// Webhook requests per stack and client address.
    pub hook_limiter: Arc<LoginLimiter>,
    /// Webhook requests per client address, across stacks.
    pub hook_address_limiter: Arc<LoginLimiter>,
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
            swarm_through: None,
            entitlements: Arc::new(fleetops_core::edition::Community),
            extra_grants: Arc::new(fleetops_core::access::NoExtraGrants),
            custom_roles: Arc::new(fleetops_core::access::NoCustomRoles),
            external_auth: Arc::new(crate::external::NoExternalAuth),
            backup_hooks: Arc::new(crate::backup::NoBackupHooks),
            restart: Arc::new(crate::backup::NoRestart),
            staged: Arc::default(),
            stack_locks: Arc::default(),
            git_syncs: Arc::default(),
            hook_limiter: Arc::new(LoginLimiter::new(
                crate::routes::stack_hooks::HOOK_MAX_REQUESTS,
                crate::routes::stack_hooks::HOOK_WINDOW,
            )),
            hook_address_limiter: Arc::new(LoginLimiter::new(
                crate::routes::stack_hooks::HOOK_MAX_PER_ADDRESS,
                crate::routes::stack_hooks::HOOK_WINDOW,
            )),
        }
    }

    /// Replaces the Community entitlements (Enterprise builds).
    pub fn with_entitlements(mut self, e: Arc<dyn fleetops_core::edition::Entitlements>) -> Self {
        self.entitlements = e;
        self
    }

    pub fn with_extra_grants(mut self, g: Arc<dyn fleetops_core::access::ExtraGrants>) -> Self {
        self.extra_grants = g;
        self
    }

    pub fn with_custom_roles(mut self, r: Arc<dyn fleetops_core::access::CustomRoles>) -> Self {
        self.custom_roles = r;
        self
    }

    pub fn with_external_auth(mut self, a: Arc<dyn crate::external::ExternalAuth>) -> Self {
        self.external_auth = a;
        self
    }

    pub fn with_backup_hooks(mut self, h: Arc<dyn crate::backup::BackupHooks>) -> Self {
        self.backup_hooks = h;
        self
    }

    pub fn with_restart(mut self, r: Arc<dyn crate::backup::Restart>) -> Self {
        self.restart = r;
        self
    }

    pub fn with_swarm_through(mut self, f: SwarmThrough) -> Self {
        self.swarm_through = Some(f);
        self
    }

    /// Shares `agents` with the env factory, so agent environments see the
    /// sessions the connect handler registers.
    pub fn with_agents(mut self, agents: Arc<fleetops_tunnel::registry::Registry>) -> Self {
        self.agents = agents;
        self
    }
}
