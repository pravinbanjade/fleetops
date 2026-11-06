//! One message, in each destination's format. Pure: no I/O.

use super::model::{image_link, security_link, CveLine, ImageLine, Message};
use crate::scan::model::Severity;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

fn sev(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "Critical",
        Severity::High => "High",
        Severity::Medium => "Medium",
        Severity::Low => "Low",
        Severity::Unknown => "Unknown",
    }
}

fn image_lines(images: &[ImageLine], more: u32) -> Vec<String> {
    let mut v: Vec<String> = images
        .iter()
        .map(|i| {
            let run = if i.running > 0 {
                format!("{} running", i.running)
            } else {
                "not running".into()
            };
            format!("{} — {} ({run})", i.image_ref, i.envs.join(", "))
        })
        .collect();
    if more > 0 {
        v.push(format!("and {more} more"));
    }
    v
}

fn cve_lines(cves: &[CveLine], more: u32) -> Vec<String> {
    let mut v: Vec<String> = cves
        .iter()
        .map(|c| {
            let fix = c
                .fixed_version
                .as_deref()
                .map_or("no fix".to_string(), |f| format!("fix {f}"));
            format!(
                "{} ({}) — {} {} → {fix}",
                c.cve,
                sev(c.severity),
                c.pkg_name,
                c.installed_version
            )
        })
        .collect();
    if more > 0 {
        v.push(format!("and {more} more"));
    }
    v
}

/// The lines under the title, the same in every format.
pub fn body_lines(m: &Message) -> Vec<String> {
    match m {
        Message::NewCve {
            packages,
            images,
            more,
            title,
            ..
        } => {
            let mut v = Vec::new();
            if let Some(t) = title {
                v.push(t.clone());
            }
            let pk: Vec<String> = packages
                .iter()
                .map(|(p, ver)| format!("{p} {ver}"))
                .collect();
            v.push(format!("Package: {}", pk.join(", ")));
            v.push("Affected images:".into());
            v.extend(
                image_lines(images, *more)
                    .into_iter()
                    .map(|l| format!("• {l}")),
            );
            v
        }
        Message::NewImage {
            envs,
            running,
            cves,
            more,
            ..
        } => {
            let mut v = vec![format!("Runs in {} ({running} running)", envs.join(", "))];
            v.extend(cve_lines(cves, *more).into_iter().map(|l| format!("• {l}")));
            v
        }
    }
}

fn link(m: &Message, public: Option<&str>) -> Option<String> {
    let p = public?;
    Some(match m {
        Message::NewImage { image_ref, .. } => image_link(p, image_ref),
        Message::NewCve { images, .. } => match images.as_slice() {
            [one] => image_link(p, &one.image_ref),
            many => {
                let mut envs: Vec<String> = many.iter().flat_map(|i| i.envs.clone()).collect();
                envs.sort();
                envs.dedup();
                security_link(p, &envs)
            }
        },
    })
}

fn worst(m: &Message) -> Severity {
    match m {
        Message::NewCve { severity, .. } => *severity,
        Message::NewImage { cves, .. } => cves
            .iter()
            .map(|c| c.severity)
            .max()
            .unwrap_or(Severity::Unknown),
    }
}

/// `s` cut to at most `max` characters, ending in "…" when cut.
pub fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Block Kit: header text at most 150 characters, section text 3000.
pub fn slack(m: &Message, public: Option<&str>) -> Value {
    let mut blocks = vec![
        json!({"type": "header", "text": {"type": "plain_text", "text": cut(&m.title(), 150), "emoji": false}}),
        json!({"type": "section", "text": {"type": "mrkdwn", "text": cut(&body_lines(m).join("\n"), 3000)}}),
        json!({"type": "context", "elements": [{"type": "mrkdwn", "text": format!("FleetOps rule: {}", m.rule().name)}]}),
    ];
    if let Some(url) = link(m, public) {
        blocks.push(json!({"type": "actions", "elements": [
            {"type": "button", "text": {"type": "plain_text", "text": "Open in FleetOps"}, "url": url}
        ]}));
    }
    json!({"text": m.title(), "blocks": blocks})
}

pub fn teams(m: &Message, public: Option<&str>) -> Value {
    let mut body = vec![
        json!({"type": "TextBlock", "text": m.title(), "weight": "Bolder", "size": "Medium", "wrap": true}),
    ];
    for l in body_lines(m) {
        body.push(json!({"type": "TextBlock", "text": l, "wrap": true, "spacing": "Small"}));
    }
    body.push(json!({"type": "TextBlock", "text": format!("FleetOps rule: {}", m.rule().name), "isSubtle": true, "size": "Small"}));
    let mut card = json!({
        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
        "type": "AdaptiveCard",
        "version": "1.4",
        "body": body,
    });
    if let Some(url) = link(m, public) {
        card["actions"] =
            json!([{"type": "Action.OpenUrl", "title": "Open in FleetOps", "url": url}]);
    }
    json!({"type": "message", "attachments": [{
        "contentType": "application/vnd.microsoft.card.adaptive",
        "content": card,
    }]})
}

pub fn discord(m: &Message, public: Option<&str>) -> Value {
    let color = match worst(m) {
        Severity::Critical => 0xFF6B6B,
        Severity::High => 0xE5A93B,
        _ => 0x949494,
    };
    let mut embed = json!({
        "title": cut(&m.title(), 256),
        "description": cut(&body_lines(m).join("\n"), 4096),
        "color": color,
        "footer": {"text": format!("FleetOps rule: {}", m.rule().name)},
    });
    if let Some(url) = link(m, public) {
        embed["url"] = json!(url);
    }
    json!({"embeds": [embed]})
}

pub fn pagerduty(m: &Message, routing_key: &str, public: Option<&str>) -> Value {
    let severity = match worst(m) {
        Severity::Critical => "critical",
        Severity::High => "error",
        Severity::Medium => "warning",
        _ => "info",
    };
    let dedup = match m {
        Message::NewCve { rule, cve, .. } => format!("fleetops:{}:{cve}", rule.id),
        Message::NewImage {
            rule, repository, ..
        } => format!("fleetops:{}:image:{repository}", rule.id),
    };
    let mut ev = json!({
        "routing_key": routing_key,
        "event_action": "trigger",
        "dedup_key": dedup,
        "payload": {
            "summary": m.title(),
            "source": "fleetops",
            "severity": severity,
            "custom_details": generic(m),
        },
    });
    if let Some(url) = link(m, public) {
        ev["links"] = json!([{"href": url, "text": "Open in FleetOps"}]);
    }
    ev
}

/// The documented, versioned body for your own tooling.
pub fn generic(m: &Message) -> Value {
    match m {
        Message::NewCve {
            rule,
            cve,
            severity,
            fixed_version,
            title,
            packages,
            images,
            more,
            ..
        } => json!({
            "version": 1,
            "event": "cve.new",
            "rule": {"id": rule.id, "name": rule.name},
            "cve": {"id": cve, "severity": severity, "fixed_version": fixed_version, "title": title,
                    "packages": packages.iter().map(|(p, v)| json!({"name": p, "installed_version": v})).collect::<Vec<_>>()},
            "images": images,
            "more": more,
        }),
        Message::NewImage {
            rule,
            repository,
            image_ref,
            envs,
            running,
            cves,
            more,
            ..
        } => json!({
            "version": 1,
            "event": "image.new",
            "rule": {"id": rule.id, "name": rule.name},
            "image": {"repository": repository, "image_ref": image_ref, "envs": envs, "running": running},
            "cves": cves,
            "more": more,
        }),
    }
}

/// `sha256=<hex>` of HMAC-SHA256 over `body`.
pub fn signature(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// (subject, plain text, HTML).
pub fn email(m: &Message, public: Option<&str>) -> (String, String, String) {
    let lines = body_lines(m);
    let url = link(m, public);
    let mut text = lines.join("\n");
    text.push_str(&format!("\n\nFleetOps rule: {}", m.rule().name));
    if let Some(u) = &url {
        text.push_str(&format!("\nOpen in FleetOps: {u}"));
    }
    let mut html = format!("<h2>{}</h2>", esc(&m.title()));
    let (head, list): (Vec<&String>, Vec<&String>) =
        lines.iter().partition(|l| !l.starts_with("• "));
    for h in head {
        html.push_str(&format!("<p>{}</p>", esc(h)));
    }
    html.push_str("<ul>");
    for l in list {
        html.push_str(&format!("<li>{}</li>", esc(l.trim_start_matches("• "))));
    }
    html.push_str("</ul>");
    html.push_str(&format!(
        "<p style=\"color:#949494\">FleetOps rule: {}</p>",
        esc(&m.rule().name)
    ));
    if let Some(u) = &url {
        html.push_str(&format!(
            "<p><a href=\"{}\">Open in FleetOps</a></p>",
            esc(u)
        ));
    }
    (m.title(), text, html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::model::{CveLine, ImageLine, RuleRef};

    fn cve() -> Message {
        Message::NewCve {
            rule: RuleRef {
                id: 3,
                name: "Prod → on-call".into(),
            },
            cve: "CVE-2024-5535".into(),
            severity: Severity::Critical,
            fixed_version: Some("3.0.14".into()),
            title: Some("openssl: buffer overread".into()),
            packages: vec![("libssl3".into(), "3.0.11".into())],
            images: vec![ImageLine {
                image_ref: "nginx:1.25".into(),
                repository: "nginx".into(),
                envs: vec!["prod".into()],
                running: 12,
            }],
            more: 0,
            fixable_only: true,
        }
    }

    fn image() -> Message {
        Message::NewImage {
            rule: RuleRef {
                id: 3,
                name: "Prod".into(),
            },
            repository: "app/worker".into(),
            image_ref: "app/worker:1".into(),
            envs: vec!["prod".into()],
            running: 2,
            cves: vec![CveLine {
                cve: "CVE-1".into(),
                severity: Severity::High,
                pkg_name: "curl".into(),
                installed_version: "7.88".into(),
                fixed_version: None,
                title: None,
            }],
            more: 4,
            fixable_only: false,
        }
    }

    #[test]
    fn slack_block_kit_has_title_images_and_link() {
        let v = slack(&cve(), Some("https://fleet.test"));
        assert_eq!(
            v["blocks"][0]["text"]["text"],
            "CVE-2024-5535 (Critical) — fix available: 3.0.14"
        );
        let body = v["blocks"][1]["text"]["text"].as_str().unwrap();
        assert!(body.contains("nginx:1.25 — prod (12 running)"), "{body}");
        assert_eq!(
            v["blocks"][3]["elements"][0]["url"],
            "https://fleet.test/security/image?image=nginx%3A1.25"
        );
    }

    #[test]
    fn teams_is_an_adaptive_card() {
        let v = teams(&cve(), None);
        assert_eq!(
            v["attachments"][0]["contentType"],
            "application/vnd.microsoft.card.adaptive"
        );
        assert_eq!(v["attachments"][0]["content"]["type"], "AdaptiveCard");
        assert!(
            v["attachments"][0]["content"]["actions"].is_null(),
            "no link without a public URL"
        );
    }

    #[test]
    fn discord_embed_colour_follows_severity() {
        assert_eq!(discord(&cve(), None)["embeds"][0]["color"], 0xFF6B6B);
        assert_eq!(discord(&image(), None)["embeds"][0]["color"], 0xE5A93B);
    }

    #[test]
    fn pagerduty_maps_severity_and_dedup_key() {
        let v = pagerduty(&cve(), "RK", None);
        assert_eq!(v["routing_key"], "RK");
        assert_eq!(v["event_action"], "trigger");
        assert_eq!(v["payload"]["severity"], "critical");
        assert_eq!(v["dedup_key"], "fleetops:3:CVE-2024-5535");
        let i = pagerduty(&image(), "RK", None);
        assert_eq!(i["payload"]["severity"], "error");
        assert_eq!(i["dedup_key"], "fleetops:3:image:app/worker");
    }

    #[test]
    fn generic_body_is_versioned_and_signed() {
        let v = generic(&cve());
        assert_eq!(
            (v["version"].as_i64(), v["event"].as_str()),
            (Some(1), Some("cve.new"))
        );
        assert_eq!(generic(&image())["event"], "image.new");
        let key: [u8; 32] = rand::random();
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
        mac.update(b"{}");
        let want = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert_eq!(signature(&key, b"{}"), want);
    }

    #[test]
    fn email_has_subject_text_and_html() {
        let (subject, text, html) = email(&image(), Some("https://fleet.test"));
        assert_eq!(subject, image().title());
        assert!(text.contains("CVE-1 (High) — curl 7.88 → no fix"), "{text}");
        assert!(text.contains("and 4 more"));
        assert!(html.contains("<li>CVE-1 (High)"), "{html}");
        assert!(html.contains("href=\"https://fleet.test/security/image?image=app%2Fworker%3A1\""));
    }

    #[test]
    fn slack_and_discord_stay_within_limits() {
        let long = format!(
            "{}.dkr.ecr.eu-west-1.amazonaws.com/team/service@sha256:{}",
            "123456789012",
            "a".repeat(64)
        );
        let mut m = image();
        if let Message::NewImage {
            image_ref, cves, ..
        } = &mut m
        {
            *image_ref = long.clone();
            for i in 0..20 {
                cves.push(CveLine {
                    cve: format!("CVE-2026-{i:05}"),
                    severity: Severity::High,
                    pkg_name: "x".repeat(120),
                    installed_version: "1".repeat(40),
                    fixed_version: Some("2".repeat(40)),
                    title: None,
                });
            }
        }
        let s = slack(&m, None);
        assert!(
            s["blocks"][0]["text"]["text"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 150
        );
        assert!(
            s["blocks"][1]["text"]["text"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 3000
        );
        assert_eq!(
            s["text"].as_str().unwrap(),
            m.title(),
            "the full title stays in the fallback text"
        );
        let d = discord(&m, None);
        assert!(d["embeds"][0]["title"].as_str().unwrap().chars().count() <= 256);
        assert!(
            d["embeds"][0]["description"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 4096
        );
    }

    #[test]
    fn no_links_without_public_url() {
        assert!(slack(&cve(), None)["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["type"] != "actions"));
        assert!(discord(&cve(), None)["embeds"][0]["url"].is_null());
        assert!(!email(&cve(), None).1.contains("Open in FleetOps"));
    }
}
