//! Delivering a message: HTTP for chat apps and webhooks, SMTP for email.

use super::format;
use super::model::{
    Destination, DestinationKind, DestinationSecrets, Message, SmtpConfig, SmtpSecurity,
};
use async_trait::async_trait;
use fleetops_core::{Error, Result};
use serde_json::Value;
use std::time::Duration;

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const PAGERDUTY_URL: &str = "https://events.pagerduty.com/v2/enqueue";

#[async_trait]
pub trait Transport: Send + Sync {
    /// POSTs `body` as JSON; Err on a non-2xx answer.
    async fn post_json(&self, url: &str, body: &Value, headers: &[(String, String)]) -> Result<()>;
    async fn send_mail(
        &self,
        smtp: &SmtpConfig,
        to: &[String],
        subject: &str,
        text: &str,
        html: &str,
    ) -> Result<()>;
}

/// The host of a URL, which is all of it ever shown or logged.
pub fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(String::from)
}

/// What went wrong, without the URL (Slack, Teams and Discord URLs are
/// credentials).
pub fn describe(what: &str, url: &str, err: &str) -> String {
    let host = host_of(url).unwrap_or_else(|| "the destination".into());
    let clean = err.replace(url, &host);
    let clean = match url::Url::parse(url) {
        Ok(u) => {
            let mut c = clean;
            if let Some(q) = u.query() {
                c = c.replace(q, "…");
            }
            let path = u.path();
            if path.len() > 1 {
                c = c.replace(path, "/…");
            }
            c
        }
        Err(_) => clean,
    };
    format!("{what} ({host}): {clean}")
}

pub struct LiveTransport {
    http: reqwest::Client,
}

impl Default for LiveTransport {
    fn default() -> Self {
        LiveTransport {
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("an HTTP client"),
        }
    }
}

#[async_trait]
impl Transport for LiveTransport {
    async fn post_json(&self, url: &str, body: &Value, headers: &[(String, String)]) -> Result<()> {
        let bytes = serde_json::to_vec(body).map_err(|e| Error::Internal(e.to_string()))?;
        let mut req = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .header("user-agent", "FleetOps");
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let res = req.body(bytes).send().await.map_err(|e| Error::Upstream {
            status: 502,
            message: describe("could not reach", url, &e.without_url().to_string()),
        })?;
        let status = res.status();
        if status.is_success() {
            return Ok(());
        }
        let text: String = res
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect();
        Err(Error::Upstream {
            status: 502,
            message: describe(&format!("answered {}", status.as_u16()), url, text.trim()),
        })
    }

    async fn send_mail(
        &self,
        smtp: &SmtpConfig,
        to: &[String],
        subject: &str,
        text: &str,
        html: &str,
    ) -> Result<()> {
        use lettre::message::{header::ContentType, MultiPart, SinglePart};
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
        let bad = |e: String| Error::Invalid(format!("email: {e}"));
        let mut b = lettre::Message::builder()
            .from(
                smtp.from_address
                    .parse()
                    .map_err(|e| bad(format!("from address: {e}")))?,
            )
            .subject(subject);
        for r in to {
            b = b.to(r.parse().map_err(|e| bad(format!("recipient {r}: {e}")))?);
        }
        let msg = b
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html.to_string()),
                    ),
            )
            .map_err(|e| bad(e.to_string()))?;
        let builder = match smtp.security {
            SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host),
            SmtpSecurity::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)
            }
            SmtpSecurity::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &smtp.host,
            )),
        }
        .map_err(|e| bad(e.to_string()))?;
        let mut builder = builder.port(smtp.port).timeout(Some(HTTP_TIMEOUT));
        if let (Some(u), Some(p)) = (&smtp.username, &smtp.password) {
            builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
        }
        builder
            .build()
            .send(msg)
            .await
            .map(|_| ())
            .map_err(|e| Error::Upstream {
                status: 502,
                message: format!("the mail server {} refused: {e}", smtp.host),
            })
    }
}

fn need<'a>(v: &'a Option<String>, what: &str) -> Result<&'a str> {
    v.as_deref().ok_or_else(|| {
        Error::Invalid(format!(
            "this destination has no {what}; edit it and save one"
        ))
    })
}

/// Sends `msg` to `dest` in its format.
pub async fn deliver(
    t: &dyn Transport,
    dest: &Destination,
    secrets: &DestinationSecrets,
    smtp: Option<&SmtpConfig>,
    msg: &Message,
    public_url: Option<&str>,
) -> Result<()> {
    match dest.kind {
        DestinationKind::Slack => {
            t.post_json(
                need(&secrets.url, "webhook URL")?,
                &format::slack(msg, public_url),
                &[],
            )
            .await
        }
        DestinationKind::Teams => {
            t.post_json(
                need(&secrets.url, "webhook URL")?,
                &format::teams(msg, public_url),
                &[],
            )
            .await
        }
        DestinationKind::Discord => {
            t.post_json(
                need(&secrets.url, "webhook URL")?,
                &format::discord(msg, public_url),
                &[],
            )
            .await
        }
        DestinationKind::PagerDuty => {
            let key = need(&secrets.routing_key, "routing key")?;
            t.post_json(PAGERDUTY_URL, &format::pagerduty(msg, key, public_url), &[])
                .await
        }
        DestinationKind::Webhook => {
            let body = format::generic(msg);
            let mut headers = vec![];
            if let Some(s) = &secrets.signing_secret {
                let bytes =
                    serde_json::to_vec(&body).map_err(|e| Error::Internal(e.to_string()))?;
                headers.push((
                    "X-FleetOps-Signature".to_string(),
                    format::signature(s.as_bytes(), &bytes),
                ));
            }
            t.post_json(need(&secrets.url, "URL")?, &body, &headers)
                .await
        }
        DestinationKind::Email => {
            let smtp = smtp.ok_or_else(|| Error::Invalid("set up email (SMTP) first".into()))?;
            let (subject, text, html) = format::email(msg, public_url);
            t.send_mail(smtp, &dest.recipients, &subject, &text, &html)
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_never_echo_the_url() {
        let url = "https://hooks.slack.com/services/T0/B0/abcSECRET?token=abc";
        let e = describe("answered 404", url, &format!("POST {url} failed: no_team"));
        assert!(!e.contains("abcSECRET") && !e.contains("token=abc"), "{e}");
        assert!(e.contains("hooks.slack.com"), "{e}");
    }

    #[test]
    fn host_of_reads_the_host() {
        assert_eq!(
            host_of("https://outlook.office.com/webhook/x").as_deref(),
            Some("outlook.office.com")
        );
        assert_eq!(host_of("not a url"), None);
    }
}
