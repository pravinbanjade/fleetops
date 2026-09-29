//! Read-only Compose project view: containers grouped by the
//! `com.docker.compose.project` label.

use fleetops_core::docker::ContainerSummary;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeProject {
    pub name: String,
    pub services: Vec<String>,
    pub containers: usize,
    pub running: usize,
    /// running | partial | stopped
    pub status: &'static str,
}

pub fn group_projects(containers: &[ContainerSummary]) -> Vec<ComposeProject> {
    let mut by_name: BTreeMap<&str, Vec<&ContainerSummary>> = BTreeMap::new();
    for c in containers {
        if let Some(p) = c.compose_project.as_deref() {
            by_name.entry(p).or_default().push(c);
        }
    }
    by_name
        .into_iter()
        .map(|(name, members)| {
            let running = members.iter().filter(|c| c.state == "running").count();
            let mut services: Vec<String> = members
                .iter()
                .filter_map(|c| c.compose_service.clone())
                .collect();
            services.sort();
            services.dedup();
            let status = match running {
                0 => "stopped",
                n if n == members.len() => "running",
                _ => "partial",
            };
            ComposeProject {
                name: name.to_string(),
                services,
                containers: members.len(),
                running,
                status,
            }
        })
        .collect()
}

/// Container ids belonging to `project`.
pub fn project_members<'a>(
    containers: &'a [ContainerSummary],
    project: &str,
) -> Vec<&'a ContainerSummary> {
    containers
        .iter()
        .filter(|c| c.compose_project.as_deref() == Some(project))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, project: Option<&str>, service: &str, state: &str) -> ContainerSummary {
        ContainerSummary {
            id: id.into(),
            name: id.into(),
            image: "img".into(),
            image_id: "sha".into(),
            state: state.into(),
            status: String::new(),
            created: 0,
            ports: vec![],
            labels: BTreeMap::new(),
            compose_project: project.map(Into::into),
            compose_service: Some(service.into()),
            volumes: vec![],
        }
    }

    #[test]
    fn groups_and_summarises_projects() {
        let list = vec![
            c("1", Some("shop"), "web", "running"),
            c("2", Some("shop"), "db", "exited"),
            c("3", Some("blog"), "web", "running"),
            c("4", Some("blog"), "web", "running"),
            c("5", None, "x", "running"),
            c("6", Some("old"), "job", "exited"),
        ];
        let p = group_projects(&list);
        assert_eq!(
            p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["blog", "old", "shop"]
        );
        assert_eq!(p[0].services, vec!["web"]);
        assert_eq!(
            (p[0].containers, p[0].running, p[0].status),
            (2, 2, "running")
        );
        assert_eq!(p[1].status, "stopped");
        assert_eq!(p[2].services, vec!["db", "web"]);
        assert_eq!(p[2].status, "partial");
        assert_eq!(project_members(&list, "shop").len(), 2);
        assert!(project_members(&list, "nope").is_empty());
    }
}
