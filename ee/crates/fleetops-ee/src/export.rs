//! Audit export (editions spec §4.4): CSV or NDJSON, streamed a page at a
//! time so a long log never sits in memory.

use bytes::Bytes;
use fleetops_store::{AuditEntry, Store};
use futures_util::{stream, Stream, StreamExt};
use serde::Deserialize;
use utoipa::ToSchema;

const PAGE: i64 = 1000;
pub const CSV_HEADER: &str =
    "id,ts,user_id,username,environment_id,action,target_type,target_id,result,detail\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Csv,
    Ndjson,
}

impl Format {
    pub fn mime(self) -> &'static str {
        match self {
            Format::Csv => "text/csv; charset=utf-8",
            Format::Ndjson => "application/x-ndjson",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Ndjson => "ndjson",
        }
    }
}

/// One CSV field: quoted when needed, and prefixed with `'` when a
/// spreadsheet would read it as a formula (container and user names are
/// chosen by users).
fn csv_field(out: &mut String, v: &str) {
    let v = if v.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{v}")
    } else {
        v.to_string()
    };
    if v.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&v.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(&v);
    }
}

fn write_row(out: &mut String, format: Format, e: &AuditEntry) {
    match format {
        Format::Ndjson => {
            out.push_str(&serde_json::to_string(e).expect("audit entry serialises"));
            out.push('\n');
        }
        Format::Csv => {
            let opt = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
            let fields = [
                e.id.to_string(),
                e.ts.to_string(),
                opt(e.user_id),
                e.username.clone().unwrap_or_default(),
                opt(e.environment_id),
                e.action.clone(),
                e.target_type.clone().unwrap_or_default(),
                e.target_id.clone().unwrap_or_default(),
                e.result.clone(),
                e.detail.as_ref().map(|d| d.to_string()).unwrap_or_default(),
            ];
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                csv_field(out, f);
            }
            out.push('\n');
        }
    }
}

/// The export body: the CSV header, then one chunk per page of rows.
pub fn rows(
    store: Store,
    from: Option<i64>,
    to: Option<i64>,
    format: Format,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    let header = (format == Format::Csv).then(|| Ok(Bytes::from_static(CSV_HEADER.as_bytes())));
    let pages = stream::unfold(Some(0i64), move |after| {
        let store = store.clone();
        async move {
            let after = after?;
            match store.audit_range(from, to, after, PAGE).await {
                Err(e) => Some((Err(std::io::Error::other(e.to_string())), None)),
                Ok(page) if page.is_empty() => None,
                Ok(page) => {
                    let next = page.last().map(|e| e.id);
                    let mut buf = String::new();
                    for e in &page {
                        write_row(&mut buf, format, e);
                    }
                    Some((Ok(Bytes::from(buf)), next))
                }
            }
        }
    });
    stream::iter(header).chain(pages)
}
