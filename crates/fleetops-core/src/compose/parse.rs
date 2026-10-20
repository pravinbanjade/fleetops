//! Compose YAML into a normalised [`Project`] (stacks spec §2). The file is
//! read into a JSON value, every string scalar is interpolated, and the
//! value is walked by hand so each problem carries its YAML path. Every
//! problem is collected before returning.

use super::interp::{interpolate, parse_variables};
use super::model::*;
use super::ComposeIssue;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub project: Project,
    pub warnings: Vec<ComposeIssue>,
}

/// A valid stack (Compose project) name.
pub fn validate_stack_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && name.len() <= 63;
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid stack name '{name}': use lowercase letters, digits, '_' and '-', starting with a letter or digit"
        ))
    }
}

const TOP_KEYS: &[&str] = &["name", "version", "services", "networks", "volumes"];
const SERVICE_KEYS: &[&str] = &[
    "image",
    "container_name",
    "command",
    "entrypoint",
    "environment",
    "ports",
    "volumes",
    "networks",
    "network_mode",
    "depends_on",
    "restart",
    "labels",
    "healthcheck",
    "user",
    "working_dir",
    "hostname",
    "extra_hosts",
    "dns",
    "cap_add",
    "cap_drop",
    "privileged",
    "read_only",
    "init",
    "tmpfs",
    "devices",
    "shm_size",
    "stop_signal",
    "stop_grace_period",
    "logging",
    "mem_limit",
    "cpus",
    "pull_policy",
    "deploy",
];

/// Why a well-known Compose key is refused, for a clearer message than
/// "unknown key".
fn unsupported_reason(key: &str) -> Option<&'static str> {
    Some(match key {
        "build" => "images must come from a registry",
        "secrets" | "configs" => "use stack variables instead",
        "env_file" => "use stack variables instead",
        "extends" | "include" => "a stack is a single file",
        "profiles" => "every service in a stack is deployed",
        _ => return None,
    })
}

struct Cx<'a> {
    vars: &'a BTreeMap<String, String>,
    issues: Vec<ComposeIssue>,
    warnings: Vec<ComposeIssue>,
}

impl Cx<'_> {
    fn err(&mut self, path: &str, msg: impl Into<String>) {
        self.issues.push(ComposeIssue::new(path, msg));
    }

    /// Reports every key of `map` not in `allowed`. `x-*` keys are allowed.
    fn keys(&mut self, map: &Map<String, Value>, allowed: &[&str], path: &str) {
        for k in map.keys() {
            if allowed.contains(&k.as_str()) || k.starts_with("x-") {
                continue;
            }
            let p = join(path, k);
            match unsupported_reason(k) {
                Some(why) => self.err(&p, format!("'{k}' is not supported: {why}")),
                None => self.err(&p, format!("unknown or unsupported key '{k}'")),
            }
        }
    }

    fn map<'v>(&mut self, v: &'v Value, path: &str) -> Option<&'v Map<String, Value>> {
        match v {
            Value::Object(m) => Some(m),
            _ => {
                self.err(path, "must be a mapping");
                None
            }
        }
    }

    /// A scalar as a string (numbers and booleans included).
    fn string(&mut self, v: &Value, path: &str) -> Option<String> {
        match scalar(v) {
            Some(s) => Some(s),
            None => {
                self.err(path, "must be a string");
                None
            }
        }
    }

    fn opt_string(&mut self, m: &Map<String, Value>, key: &str, path: &str) -> Option<String> {
        match m.get(key) {
            None | Some(Value::Null) => None,
            Some(v) => self.string(v, &join(path, key)),
        }
    }

    fn bool(&mut self, m: &Map<String, Value>, key: &str, path: &str) -> Option<bool> {
        match m.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::String(s)) if s == "true" || s == "false" => Some(s == "true"),
            Some(_) => {
                self.err(&join(path, key), "must be true or false");
                None
            }
        }
    }

    /// A string or a list of strings, as a list.
    fn string_list(&mut self, v: &Value, path: &str) -> Vec<String> {
        match v {
            Value::Null => vec![],
            Value::Array(items) => items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| self.string(item, &idx(path, i)))
                .collect(),
            other => self.string(other, path).into_iter().collect(),
        }
    }

    /// A mapping or a list of `k=v` (or `k` alone, a `None` value).
    fn kv(&mut self, v: &Value, path: &str, sep: char) -> BTreeMap<String, Option<String>> {
        let mut out = BTreeMap::new();
        match v {
            Value::Null => {}
            Value::Object(m) => {
                for (k, val) in m {
                    let val = match val {
                        Value::Null => None,
                        other => self.string(other, &join(path, k)),
                    };
                    out.insert(k.clone(), val);
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    if let Some(s) = self.string(item, &idx(path, i)) {
                        match s.split_once(sep) {
                            Some((k, val)) => out.insert(k.to_string(), Some(val.to_string())),
                            None => out.insert(s, None),
                        };
                    }
                }
            }
            _ => self.err(path, "must be a mapping or a list"),
        }
        out
    }

    fn string_map(&mut self, v: Option<&Value>, path: &str) -> BTreeMap<String, String> {
        match v {
            None => BTreeMap::new(),
            Some(v) => self
                .kv(v, path, '=')
                .into_iter()
                .map(|(k, val)| (k, val.unwrap_or_default()))
                .collect(),
        }
    }

    fn duration(&mut self, m: &Map<String, Value>, key: &str, path: &str) -> Option<i64> {
        let p = join(path, key);
        let s = self.opt_string(m, key, path)?;
        match parse_duration(&s) {
            Some(d) => Some(d),
            None => {
                self.err(&p, format!("invalid duration '{s}'"));
                None
            }
        }
    }

    fn size(&mut self, m: &Map<String, Value>, key: &str, path: &str) -> Option<i64> {
        let p = join(path, key);
        let s = self.opt_string(m, key, path)?;
        match parse_size(&s) {
            Some(d) => Some(d),
            None => {
                self.err(&p, format!("invalid size '{s}'"));
                None
            }
        }
    }

    /// Interpolates every string scalar under `v`, in place.
    fn interpolate_all(&mut self, v: &mut Value, path: &str) {
        match v {
            Value::String(s) => match interpolate(s, self.vars, path, &mut self.warnings) {
                Ok(out) => *s = out,
                Err(e) => self.issues.push(e),
            },
            Value::Array(items) => {
                for (i, item) in items.iter_mut().enumerate() {
                    self.interpolate_all(item, &idx(path, i));
                }
            }
            Value::Object(m) => {
                for (k, item) in m.iter_mut() {
                    self.interpolate_all(item, &join(path, k));
                }
            }
            _ => {}
        }
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn idx(path: &str, i: usize) -> String {
    format!("{path}[{i}]")
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `1h2m3s`, `10s`, `500ms` (rounded down to seconds), or bare seconds.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    let mut total_ms: i64 = 0;
    let mut rest = s;
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let n_end = rest.find(|c: char| !c.is_ascii_digit() && c != '.')?;
        let n: f64 = rest[..n_end].parse().ok()?;
        rest = &rest[n_end..];
        let u_end = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let unit = &rest[..u_end];
        rest = &rest[u_end..];
        let ms = match unit {
            "ms" => 1.0,
            "s" => 1_000.0,
            "m" => 60_000.0,
            "h" => 3_600_000.0,
            "us" | "µs" => 0.001,
            "ns" => 0.000_001,
            _ => return None,
        };
        total_ms += (n * ms) as i64;
    }
    Some(total_ms / 1000)
}

/// `512m`, `1g`, `100kb`, `1024b`, or bare bytes (1024-based units).
pub fn parse_size(s: &str) -> Option<i64> {
    let s = s.trim().to_ascii_lowercase();
    let n_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let n: f64 = s[..n_end].parse().ok()?;
    let mult: f64 = match &s[n_end..] {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as i64)
}

/// Splits a command string the way a POSIX shell would (quotes and
/// backslashes), without expansion.
pub fn shell_split(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unclosed single quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => cur.push(c),
                            Some(c) => {
                                cur.push('\\');
                                cur.push(c);
                            }
                            None => return Err("unclosed double quote".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unclosed double quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(c) = chars.next() {
                    cur.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

/// Parses a Compose file for the stack `stack`, with `variables` as
/// `KEY=value` lines.
pub fn parse(yaml: &str, variables: &[String], stack: &str) -> Result<Parsed, Vec<ComposeIssue>> {
    let vars = parse_variables(variables).map_err(|e| vec![e])?;
    let mut cx = Cx {
        vars: &vars,
        issues: vec![],
        warnings: vec![],
    };
    if let Err(e) = validate_stack_name(stack) {
        cx.err("name", e);
    }
    let mut root: Value = match serde_saphyr::from_str(yaml) {
        Ok(v) => v,
        Err(e) => return Err(vec![ComposeIssue::new("", format!("invalid YAML: {e}"))]),
    };
    cx.interpolate_all(&mut root, "");
    let project = project(&mut cx, &root, stack);
    if cx.issues.is_empty() {
        Ok(Parsed {
            project: project.expect("no issues means a project"),
            warnings: cx.warnings,
        })
    } else {
        Err(cx.issues)
    }
}

fn project(cx: &mut Cx, root: &Value, stack: &str) -> Option<Project> {
    let top = cx.map(root, "")?;
    cx.keys(top, TOP_KEYS, "");
    if let Some(n) = cx.opt_string(top, "name", "") {
        if n != stack {
            cx.err(
                "name",
                format!("name '{n}' must match the stack name '{stack}'"),
            );
        }
    }
    let mut networks = BTreeMap::new();
    if let Some(v) = top.get("networks").filter(|v| !v.is_null()) {
        if let Some(m) = cx.map(v, "networks") {
            for (k, nv) in m {
                if let Some(n) = network(cx, stack, k, nv) {
                    networks.insert(k.clone(), n);
                }
            }
        }
    }
    let mut volumes = BTreeMap::new();
    if let Some(v) = top.get("volumes").filter(|v| !v.is_null()) {
        if let Some(m) = cx.map(v, "volumes") {
            for (k, vv) in m {
                if let Some(vol) = volume(cx, stack, k, vv) {
                    volumes.insert(k.clone(), vol);
                }
            }
        }
    }
    let mut services = BTreeMap::new();
    match top.get("services") {
        None | Some(Value::Null) => cx.err("services", "a stack needs at least one service"),
        Some(v) => {
            if let Some(m) = cx.map(v, "services") {
                if m.is_empty() {
                    cx.err("services", "a stack needs at least one service");
                }
                for (k, sv) in m {
                    if let Some(s) = service(cx, k, sv) {
                        services.insert(k.clone(), s);
                    }
                }
            }
        }
    }
    let uses_default = services
        .values()
        .any(|s| s.networks.contains_key("default"));
    if uses_default && !networks.contains_key("default") {
        networks.insert(
            "default".into(),
            Network {
                name: format!("{stack}_default"),
                ..Default::default()
            },
        );
    }
    references(cx, &services, &networks, &volumes);
    Some(Project {
        name: stack.to_string(),
        services,
        networks,
        volumes,
    })
}

fn network(cx: &mut Cx, stack: &str, key: &str, v: &Value) -> Option<Network> {
    let path = join("networks", key);
    if v.is_null() {
        return Some(Network {
            name: format!("{stack}_{key}"),
            ..Default::default()
        });
    }
    let m = cx.map(v, &path)?;
    cx.keys(
        m,
        &[
            "driver",
            "driver_opts",
            "internal",
            "attachable",
            "labels",
            "external",
            "name",
        ],
        &path,
    );
    let external = cx.bool(m, "external", &path).unwrap_or(false);
    let name = cx.opt_string(m, "name", &path).unwrap_or_else(|| {
        if external {
            key.to_string()
        } else {
            format!("{stack}_{key}")
        }
    });
    Some(Network {
        external,
        name,
        driver: cx.opt_string(m, "driver", &path),
        driver_opts: cx.string_map(m.get("driver_opts"), &join(&path, "driver_opts")),
        internal: cx.bool(m, "internal", &path).unwrap_or(false),
        attachable: cx.bool(m, "attachable", &path).unwrap_or(false),
        labels: cx.string_map(m.get("labels"), &join(&path, "labels")),
    })
}

fn volume(cx: &mut Cx, stack: &str, key: &str, v: &Value) -> Option<Volume> {
    let path = join("volumes", key);
    if v.is_null() {
        return Some(Volume {
            name: format!("{stack}_{key}"),
            ..Default::default()
        });
    }
    let m = cx.map(v, &path)?;
    cx.keys(
        m,
        &["driver", "driver_opts", "labels", "external", "name"],
        &path,
    );
    let external = cx.bool(m, "external", &path).unwrap_or(false);
    let name = cx.opt_string(m, "name", &path).unwrap_or_else(|| {
        if external {
            key.to_string()
        } else {
            format!("{stack}_{key}")
        }
    });
    Some(Volume {
        external,
        name,
        driver: cx.opt_string(m, "driver", &path),
        driver_opts: cx.string_map(m.get("driver_opts"), &join(&path, "driver_opts")),
        labels: cx.string_map(m.get("labels"), &join(&path, "labels")),
    })
}

fn command(cx: &mut Cx, m: &Map<String, Value>, key: &str, path: &str) -> Option<Vec<String>> {
    let p = join(path, key);
    match m.get(key)? {
        Value::Null => None,
        Value::Array(_) => Some(cx.string_list(&m[key], &p)),
        v => {
            let s = cx.string(v, &p)?;
            match shell_split(&s) {
                Ok(words) => Some(words),
                Err(e) => {
                    cx.err(&p, e);
                    None
                }
            }
        }
    }
}

fn service(cx: &mut Cx, name: &str, v: &Value) -> Option<Service> {
    let path = join("services", name);
    let m = cx.map(v, &path)?;
    cx.keys(m, SERVICE_KEYS, &path);
    let image = cx.opt_string(m, "image", &path).unwrap_or_default();
    if image.trim().is_empty() {
        cx.err(&join(&path, "image"), "image is required");
    } else if image.ends_with(':') {
        cx.err(
            &join(&path, "image"),
            format!("image '{image}' has an empty tag"),
        );
    }
    let environment = match m.get("environment") {
        None => BTreeMap::new(),
        Some(v) => {
            let p = join(&path, "environment");
            let mut out = BTreeMap::new();
            for (k, val) in cx.kv(v, &p, '=') {
                match val {
                    Some(val) => {
                        out.insert(k, val);
                    }
                    // `KEY` alone takes the stack variable of that name.
                    None => match cx.vars.get(&k) {
                        Some(val) => {
                            out.insert(k, val.clone());
                        }
                        None => cx.warnings.push(ComposeIssue::new(
                            join(&p, &k),
                            format!("variable {k} is not set; leaving it out"),
                        )),
                    },
                }
            }
            out
        }
    };
    let ports = match m.get("ports") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, p)| port(cx, p, &idx(&join(&path, "ports"), i)))
            .collect(),
        Some(_) => {
            cx.err(&join(&path, "ports"), "must be a list");
            vec![]
        }
    };
    let mounts = match m.get("volumes") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, p)| mount(cx, p, &idx(&join(&path, "volumes"), i)))
            .collect(),
        Some(_) => {
            cx.err(&join(&path, "volumes"), "must be a list");
            vec![]
        }
    };
    let network_mode = cx.opt_string(m, "network_mode", &path);
    if let Some(mode) = &network_mode {
        let ok =
            matches!(mode.as_str(), "bridge" | "host" | "none") || mode.starts_with("service:");
        if !ok {
            cx.err(
                &join(&path, "network_mode"),
                format!("network_mode '{mode}' is not supported: use bridge, host, none or service:<name>"),
            );
        }
        if m.get("networks").is_some_and(|n| !n.is_null()) {
            cx.err(
                &join(&path, "networks"),
                "networks cannot be set with network_mode",
            );
        }
    }
    let mut networks = BTreeMap::new();
    match m.get("networks") {
        None | Some(Value::Null) => {
            if network_mode.is_none() {
                networks.insert("default".to_string(), ServiceNetwork::default());
            }
        }
        Some(Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                if let Some(n) = cx.string(item, &idx(&join(&path, "networks"), i)) {
                    networks.insert(n, ServiceNetwork::default());
                }
            }
        }
        Some(Value::Object(nm)) => {
            for (k, nv) in nm {
                let np = join(&join(&path, "networks"), k);
                let sn = match nv {
                    Value::Null => ServiceNetwork::default(),
                    other => match cx.map(other, &np) {
                        Some(o) => {
                            cx.keys(o, &["aliases", "ipv4_address"], &np);
                            ServiceNetwork {
                                aliases: o
                                    .get("aliases")
                                    .map(|a| cx.string_list(a, &join(&np, "aliases")))
                                    .unwrap_or_default(),
                                ipv4_address: cx.opt_string(o, "ipv4_address", &np),
                            }
                        }
                        None => continue,
                    },
                };
                networks.insert(k.clone(), sn);
            }
        }
        Some(_) => cx.err(&join(&path, "networks"), "must be a list or a mapping"),
    }
    let mut depends_on = BTreeMap::new();
    match m.get("depends_on") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                if let Some(d) = cx.string(item, &idx(&join(&path, "depends_on"), i)) {
                    depends_on.insert(d, DependsCondition::Started);
                }
            }
        }
        Some(Value::Object(dm)) => {
            for (k, dv) in dm {
                let dp = join(&join(&path, "depends_on"), k);
                let cond = match dv {
                    Value::Null => DependsCondition::Started,
                    other => match cx.map(other, &dp) {
                        Some(o) => {
                            cx.keys(o, &["condition"], &dp);
                            match cx.opt_string(o, "condition", &dp).as_deref() {
                                None | Some("service_started") => DependsCondition::Started,
                                Some("service_healthy") => DependsCondition::Healthy,
                                Some("service_completed_successfully") => {
                                    DependsCondition::CompletedSuccessfully
                                }
                                Some(c) => {
                                    cx.err(
                                        &join(&dp, "condition"),
                                        format!("unknown condition '{c}'"),
                                    );
                                    continue;
                                }
                            }
                        }
                        None => continue,
                    },
                };
                depends_on.insert(k.clone(), cond);
            }
        }
        Some(_) => cx.err(&join(&path, "depends_on"), "must be a list or a mapping"),
    }
    let restart = cx.opt_string(m, "restart", &path);
    if let Some(r) = &restart {
        let ok = matches!(
            r.as_str(),
            "no" | "always" | "unless-stopped" | "on-failure"
        ) || r
            .strip_prefix("on-failure:")
            .is_some_and(|n| n.parse::<u32>().is_ok());
        if !ok {
            cx.err(
                &join(&path, "restart"),
                format!("invalid restart policy '{r}'"),
            );
        }
    }
    let healthcheck = m
        .get("healthcheck")
        .and_then(|h| healthcheck(cx, h, &join(&path, "healthcheck")));
    let logging = match m.get("logging") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let lp = join(&path, "logging");
            cx.map(v, &lp).map(|lm| {
                cx.keys(lm, &["driver", "options"], &lp);
                Logging {
                    driver: cx
                        .opt_string(lm, "driver", &lp)
                        .unwrap_or_else(|| "json-file".into()),
                    options: cx.string_map(lm.get("options"), &join(&lp, "options")),
                }
            })
        }
    };
    let pull_policy = match cx.opt_string(m, "pull_policy", &path).as_deref() {
        None | Some("missing") | Some("if_not_present") => PullPolicy::Missing,
        Some("always") => PullPolicy::Always,
        Some("never") => PullPolicy::Never,
        Some(p) => {
            cx.err(
                &join(&path, "pull_policy"),
                format!("invalid pull_policy '{p}'"),
            );
            PullPolicy::Missing
        }
    };
    let mut memory_bytes = cx.size(m, "mem_limit", &path);
    let mut nano_cpus = cx
        .opt_string(m, "cpus", &path)
        .and_then(|c| cpus(cx, &c, &join(&path, "cpus")));
    let mut replicas = 1u32;
    if let Some(dv) = m.get("deploy").filter(|v| !v.is_null()) {
        let dp = join(&path, "deploy");
        if let Some(dm) = cx.map(dv, &dp) {
            cx.keys(dm, &["replicas", "resources"], &dp);
            if let Some(r) = dm.get("replicas") {
                match r
                    .as_u64()
                    .or_else(|| scalar(r).and_then(|s| s.parse().ok()))
                {
                    Some(n) => replicas = n as u32,
                    None => cx.err(&join(&dp, "replicas"), "must be a whole number"),
                }
            }
            if let Some(rv) = dm.get("resources").filter(|v| !v.is_null()) {
                let rp = join(&dp, "resources");
                if let Some(rm) = cx.map(rv, &rp) {
                    cx.keys(rm, &["limits"], &rp);
                    if let Some(lv) = rm.get("limits").filter(|v| !v.is_null()) {
                        let lp = join(&rp, "limits");
                        if let Some(lm) = cx.map(lv, &lp) {
                            cx.keys(lm, &["memory", "cpus"], &lp);
                            if let Some(mb) = cx.size(lm, "memory", &lp) {
                                memory_bytes = Some(mb);
                            }
                            if let Some(c) = cx.opt_string(lm, "cpus", &lp) {
                                nano_cpus = cpus(cx, &c, &join(&lp, "cpus")).or(nano_cpus);
                            }
                        }
                    }
                }
            }
        }
    }
    let container_name = cx.opt_string(m, "container_name", &path);
    if let Some(cn) = &container_name {
        if let Err(e) = crate::docker::validate_container_name(cn) {
            cx.err(&join(&path, "container_name"), e.to_string());
        }
        if replicas > 1 {
            cx.err(
                &join(&path, "container_name"),
                "container_name cannot be used with more than one replica",
            );
        }
    }
    let extra_hosts = match m.get("extra_hosts") {
        None | Some(Value::Null) => vec![],
        Some(Value::Object(_)) => cx
            .kv(&m["extra_hosts"], &join(&path, "extra_hosts"), '=')
            .into_iter()
            .map(|(h, ip)| format!("{h}:{}", ip.unwrap_or_default()))
            .collect(),
        Some(v) => cx.string_list(v, &join(&path, "extra_hosts")),
    };
    let list = |cx: &mut Cx, key: &str| {
        m.get(key)
            .map(|v| cx.string_list(v, &join(&path, key)))
            .unwrap_or_default()
    };
    Some(Service {
        name: name.to_string(),
        image,
        container_name,
        command: command(cx, m, "command", &path),
        entrypoint: command(cx, m, "entrypoint", &path),
        environment,
        ports,
        mounts,
        networks,
        network_mode,
        depends_on,
        restart,
        labels: cx.string_map(m.get("labels"), &join(&path, "labels")),
        healthcheck,
        user: cx.opt_string(m, "user", &path),
        working_dir: cx.opt_string(m, "working_dir", &path),
        hostname: cx.opt_string(m, "hostname", &path),
        extra_hosts,
        dns: list(cx, "dns"),
        cap_add: list(cx, "cap_add"),
        cap_drop: list(cx, "cap_drop"),
        tmpfs: list(cx, "tmpfs"),
        devices: list(cx, "devices"),
        privileged: cx.bool(m, "privileged", &path).unwrap_or(false),
        read_only: cx.bool(m, "read_only", &path).unwrap_or(false),
        init: cx.bool(m, "init", &path),
        shm_size: cx.size(m, "shm_size", &path),
        stop_signal: cx.opt_string(m, "stop_signal", &path),
        stop_grace_period_secs: cx.duration(m, "stop_grace_period", &path),
        logging,
        memory_bytes,
        nano_cpus,
        pull_policy,
        replicas,
    })
}

fn cpus(cx: &mut Cx, s: &str, path: &str) -> Option<i64> {
    match s.trim().parse::<f64>() {
        Ok(c) if c > 0.0 => Some((c * 1e9) as i64),
        _ => {
            cx.err(path, format!("invalid cpus '{s}'"));
            None
        }
    }
}

fn port_number(cx: &mut Cx, s: &str, path: &str) -> Option<u16> {
    if s.contains('-') {
        cx.err(path, "port ranges are not supported");
        return None;
    }
    match s.parse::<u16>() {
        Ok(n) if n > 0 => Some(n),
        _ => {
            cx.err(path, format!("invalid port '{s}'"));
            None
        }
    }
}

fn port(cx: &mut Cx, v: &Value, path: &str) -> Option<Port> {
    if let Value::Object(m) = v {
        cx.keys(
            m,
            &["target", "published", "host_ip", "protocol", "mode"],
            path,
        );
        let target = cx.opt_string(m, "target", path);
        let Some(target) = target else {
            cx.err(&join(path, "target"), "target is required");
            return None;
        };
        let target = port_number(cx, &target, &join(path, "target"))?;
        let published = match cx.opt_string(m, "published", path) {
            Some(p) => Some(port_number(cx, &p, &join(path, "published"))?),
            None => None,
        };
        let protocol = cx
            .opt_string(m, "protocol", path)
            .unwrap_or_else(|| "tcp".into());
        return port_protocol(cx, protocol, path).map(|protocol| Port {
            target,
            published,
            host_ip: cx.opt_string(m, "host_ip", path),
            protocol,
        });
    }
    let s = cx.string(v, path)?;
    let (spec, protocol) = match s.rsplit_once('/') {
        Some((a, p)) => (a.to_string(), p.to_string()),
        None => (s.clone(), "tcp".to_string()),
    };
    let protocol = port_protocol(cx, protocol, path)?;
    // An IPv6 host address is bracketed: `[::1]:8080:80`.
    let (host_ip, rest) = match spec.strip_prefix('[') {
        Some(r) => match r.split_once("]:") {
            Some((ip, rest)) => (Some(ip.to_string()), rest.to_string()),
            None => {
                cx.err(path, format!("invalid port '{s}'"));
                return None;
            }
        },
        None => (None, spec),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let (host_ip, published, target) = match (host_ip, parts.as_slice()) {
        (None, [t]) => (None, None, *t),
        (h, [p, t]) => (h, Some(*p), *t),
        (None, [h, p, t]) => (Some(h.to_string()), Some(*p), *t),
        _ => {
            cx.err(path, format!("invalid port '{s}'"));
            return None;
        }
    };
    let target = port_number(cx, target, path)?;
    let published = match published.filter(|p| !p.is_empty()) {
        Some(p) => Some(port_number(cx, p, path)?),
        None => None,
    };
    Some(Port {
        target,
        published,
        host_ip,
        protocol,
    })
}

fn port_protocol(cx: &mut Cx, p: String, path: &str) -> Option<String> {
    if matches!(p.as_str(), "tcp" | "udp" | "sctp") {
        Some(p)
    } else {
        cx.err(path, format!("invalid port protocol '{p}'"));
        None
    }
}

fn is_bind_source(s: &str) -> bool {
    s.starts_with('/') || s.starts_with('.') || s.starts_with('~')
}

const RELATIVE_BIND: &str = "relative bind paths are not supported; use an absolute host path";

fn mount(cx: &mut Cx, v: &Value, path: &str) -> Option<Mount> {
    if let Value::Object(m) = v {
        cx.keys(m, &["type", "source", "target", "read_only"], path);
        let target = cx.opt_string(m, "target", path);
        let Some(target) = target else {
            cx.err(&join(path, "target"), "target is required");
            return None;
        };
        let source = cx.opt_string(m, "source", path).unwrap_or_default();
        let read_only = cx.bool(m, "read_only", path).unwrap_or(false);
        return match cx.opt_string(m, "type", path).as_deref() {
            Some("volume") => Some(Mount::Volume {
                source,
                target,
                read_only,
            }),
            Some("bind") => {
                if !source.starts_with('/') {
                    cx.err(&join(path, "source"), RELATIVE_BIND);
                    return None;
                }
                Some(Mount::Bind {
                    source,
                    target,
                    read_only,
                })
            }
            Some("tmpfs") => Some(Mount::Tmpfs { target }),
            Some(t) => {
                cx.err(&join(path, "type"), format!("unsupported mount type '{t}'"));
                None
            }
            None => {
                cx.err(&join(path, "type"), "type is required");
                None
            }
        };
    }
    let s = cx.string(v, path)?;
    let parts: Vec<&str> = s.split(':').collect();
    let (source, target, mode) = match parts.as_slice() {
        [t] => ("", *t, None),
        [s, t] => (*s, *t, None),
        [s, t, m] => (*s, *t, Some(*m)),
        _ => {
            cx.err(path, format!("invalid volume '{s}'"));
            return None;
        }
    };
    let read_only = match mode {
        None | Some("rw") => false,
        Some("ro") => true,
        Some(m) => {
            cx.err(path, format!("unsupported volume mode '{m}': use ro or rw"));
            return None;
        }
    };
    if source.is_empty() {
        return Some(Mount::Volume {
            source: String::new(),
            target: target.to_string(),
            read_only,
        });
    }
    if is_bind_source(source) {
        if !source.starts_with('/') {
            cx.err(path, RELATIVE_BIND);
            return None;
        }
        return Some(Mount::Bind {
            source: source.to_string(),
            target: target.to_string(),
            read_only,
        });
    }
    Some(Mount::Volume {
        source: source.to_string(),
        target: target.to_string(),
        read_only,
    })
}

fn healthcheck(cx: &mut Cx, v: &Value, path: &str) -> Option<Healthcheck> {
    let m = cx.map(v, path)?;
    cx.keys(
        m,
        &[
            "test",
            "interval",
            "timeout",
            "retries",
            "start_period",
            "disable",
        ],
        path,
    );
    let mut h = Healthcheck::default();
    match m.get("test") {
        None | Some(Value::Null) => {}
        Some(Value::Array(_)) => h.test = cx.string_list(&m["test"], &join(path, "test")),
        Some(other) => {
            if let Some(s) = cx.string(other, &join(path, "test")) {
                h.test = vec!["CMD-SHELL".into(), s];
            }
        }
    }
    if h.test.first().map(String::as_str) == Some("NONE") {
        h.disable = true;
    }
    if let Some(d) = cx.duration(m, "interval", path) {
        h.interval_secs = d;
    }
    if let Some(d) = cx.duration(m, "timeout", path) {
        h.timeout_secs = d;
    }
    if let Some(d) = cx.duration(m, "start_period", path) {
        h.start_period_secs = d;
    }
    if let Some(r) = cx.opt_string(m, "retries", path) {
        match r.parse() {
            Ok(n) => h.retries = n,
            Err(_) => cx.err(&join(path, "retries"), "must be a whole number"),
        }
    }
    if cx.bool(m, "disable", path) == Some(true) {
        h.disable = true;
    }
    Some(h)
}

/// Cross-references: networks, volumes and services a service names must
/// exist, and `depends_on` must not loop.
fn references(
    cx: &mut Cx,
    services: &BTreeMap<String, Service>,
    networks: &BTreeMap<String, Network>,
    volumes: &BTreeMap<String, Volume>,
) {
    for (name, s) in services {
        let path = join("services", name);
        for n in s.networks.keys() {
            if !networks.contains_key(n) {
                cx.err(
                    &join(&join(&path, "networks"), n),
                    format!("network '{n}' is not defined under networks"),
                );
            }
        }
        for (i, mnt) in s.mounts.iter().enumerate() {
            if let Mount::Volume { source, .. } = mnt {
                if !source.is_empty() && !volumes.contains_key(source) {
                    cx.err(
                        &idx(&join(&path, "volumes"), i),
                        format!("volume '{source}' is not defined under volumes"),
                    );
                }
            }
        }
        for d in s.depends_on.keys() {
            if !services.contains_key(d) {
                cx.err(
                    &join(&join(&path, "depends_on"), d),
                    format!("service '{d}' does not exist"),
                );
            }
        }
        if let Some(target) = s
            .network_mode
            .as_deref()
            .and_then(|m| m.strip_prefix("service:"))
        {
            if !services.contains_key(target) {
                cx.err(
                    &join(&path, "network_mode"),
                    format!("service '{target}' does not exist"),
                );
            }
        }
    }
    if let Some(cycle) = find_cycle(services) {
        cx.err(
            "services",
            format!("dependency cycle: {}", cycle.join(" -> ")),
        );
    }
}

/// Dependencies of a service: `depends_on` plus `network_mode: service:x`.
pub(crate) fn deps(s: &Service) -> BTreeSet<String> {
    let mut d: BTreeSet<String> = s.depends_on.keys().cloned().collect();
    if let Some(t) = s
        .network_mode
        .as_deref()
        .and_then(|m| m.strip_prefix("service:"))
    {
        d.insert(t.to_string());
    }
    d
}

fn find_cycle(services: &BTreeMap<String, Service>) -> Option<Vec<String>> {
    fn visit(
        n: &str,
        services: &BTreeMap<String, Service>,
        stack: &mut Vec<String>,
        done: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if let Some(pos) = stack.iter().position(|s| s == n) {
            let mut c = stack[pos..].to_vec();
            c.push(n.to_string());
            return Some(c);
        }
        if done.contains(n) {
            return None;
        }
        stack.push(n.to_string());
        if let Some(s) = services.get(n) {
            for d in deps(s) {
                if let Some(c) = visit(&d, services, stack, done) {
                    return Some(c);
                }
            }
        }
        stack.pop();
        done.insert(n.to_string());
        None
    }
    let mut done = BTreeSet::new();
    for n in services.keys() {
        if let Some(c) = visit(n, services, &mut vec![], &mut done) {
            return Some(c);
        }
    }
    None
}
