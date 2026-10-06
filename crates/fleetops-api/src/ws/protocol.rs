//! Wire format of `/api/v1/ws` (spec §5.2). Text frames carry JSON; exec
//! I/O uses binary frames of `[u32 sub id, big-endian][payload]`.

use crate::tasks::TaskInfo;
use fleetops_core::env::EnvStatus;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Topic {
    Events,
    Stats,
    Logs,
    Exec,
    Task,
    /// A live list of Kubernetes objects; `id` is `<kind>/<namespace or ->`.
    #[serde(rename = "kube-watch")]
    KubeWatch,
}

/// A text frame from the client. Exec stdin may also arrive as a binary
/// frame of `[u32 sub id, big-endian][bytes]`.
#[derive(Debug, Clone, PartialEq, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum ClientMsg {
    Sub {
        sub: u32,
        topic: Topic,
        #[serde(default)]
        env: Option<String>,
        #[serde(default)]
        id: Option<String>,
        /// `logs`: [`LogOptions`](fleetops_core::streams::LogOptions);
        /// `exec`: [`ExecOptions`](fleetops_core::streams::ExecOptions).
        #[serde(default)]
        opts: Value,
    },
    Unsub {
        sub: u32,
    },
    /// Exec stdin as text, for clients that cannot send binary frames.
    Input {
        sub: u32,
        data: String,
    },
    Resize {
        sub: u32,
        cols: u16,
        rows: u16,
    },
}

/// A text frame from the server, as the builders below write it. Exec
/// output arrives as binary frames of `[u32 sub id, big-endian][bytes]`.
/// Documentation only: `tests/openapi.rs` checks each builder against it.
#[derive(Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ServerMsg {
    /// A payload for subscription `sub`: a `DockerEvent`, a `StatsSample`,
    /// an array of `LogLine`s, a task progress message, or a `WatchEvent`
    /// (kube-watch).
    Data { sub: u32, data: Value },
    /// The logs queue overflowed and `dropped` lines were skipped.
    Gap { sub: u32, dropped: u64 },
    /// The subscription is over. `reason` is `closed`, `ended`, `eof`,
    /// `exited` (with `exit_code`), `succeeded` or `failed` (with `task`),
    /// `error` (kube-watch, with `message`), or `access` (the user's access
    /// changed, with `message`).
    End {
        sub: u32,
        reason: String,
        exit_code: Option<i64>,
        task: Option<TaskInfo>,
        message: Option<String>,
    },
    /// A request failed; `sub` is null when the frame named no valid
    /// subscription. `code` is an error code as in `ErrorBody`.
    Error {
        sub: Option<u32>,
        code: String,
        message: String,
    },
    /// An environment went up or down.
    EnvStatus { env: String, status: EnvStatus },
    /// The user's access changed: what they can see may differ.
    AccessChanged,
}

/// A frame ready to go out on the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Text(String),
    Binary(Vec<u8>),
    /// A websocket ping, for detecting half-open connections.
    Ping,
}

pub fn data(sub: u32, data: Value) -> Out {
    Out::Text(json!({"sub": sub, "type": "data", "data": data}).to_string())
}

pub fn gap(sub: u32, dropped: u64) -> Out {
    Out::Text(json!({"sub": sub, "type": "gap", "dropped": dropped}).to_string())
}

pub fn end(sub: u32, reason: &str, extra: Value) -> Out {
    let mut v = json!({"sub": sub, "type": "end", "reason": reason});
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    Out::Text(v.to_string())
}

/// `sub` is None for errors about a frame that named no valid subscription.
pub fn error(sub: Option<u32>, code: &str, message: &str) -> Out {
    Out::Text(json!({"sub": sub, "type": "error", "code": code, "message": message}).to_string())
}

pub fn access_changed() -> Out {
    Out::Text(json!({"type": "access_changed"}).to_string())
}

pub fn env_status(env: &str, status: &Value) -> Out {
    Out::Text(json!({"type": "env_status", "env": env, "status": status}).to_string())
}

pub fn encode_binary(sub: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + payload.len());
    v.extend_from_slice(&sub.to_be_bytes());
    v.extend_from_slice(payload);
    v
}

pub fn decode_binary(frame: &[u8]) -> Option<(u32, &[u8])> {
    let (id, payload) = frame.split_at_checked(4)?;
    Some((u32::from_be_bytes(id.try_into().ok()?), payload))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_client_op() {
        let sub: ClientMsg = serde_json::from_str(
            r#"{"op":"sub","sub":7,"topic":"logs","env":"local","id":"web","opts":{"tail":10}}"#,
        )
        .unwrap();
        assert_eq!(
            sub,
            ClientMsg::Sub {
                sub: 7,
                topic: Topic::Logs,
                env: Some("local".into()),
                id: Some("web".into()),
                opts: json!({"tail": 10})
            }
        );
        let bare: ClientMsg =
            serde_json::from_str(r#"{"op":"sub","sub":1,"topic":"events","env":"local"}"#).unwrap();
        assert!(matches!(
            bare,
            ClientMsg::Sub {
                id: None,
                opts: Value::Null,
                ..
            }
        ));
        for (s, want) in [
            (r#"{"op":"unsub","sub":3}"#, ClientMsg::Unsub { sub: 3 }),
            (
                r#"{"op":"input","sub":3,"data":"ls\n"}"#,
                ClientMsg::Input {
                    sub: 3,
                    data: "ls\n".into(),
                },
            ),
            (
                r#"{"op":"resize","sub":3,"cols":120,"rows":40}"#,
                ClientMsg::Resize {
                    sub: 3,
                    cols: 120,
                    rows: 40,
                },
            ),
        ] {
            assert_eq!(serde_json::from_str::<ClientMsg>(s).unwrap(), want);
        }
        for bad in [
            r#"{"op":"sub","sub":1,"topic":"nope"}"#,
            r#"{"op":"explode"}"#,
            r#"{"op":"unsub","sub":-1}"#,
            "not json",
        ] {
            assert!(serde_json::from_str::<ClientMsg>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn server_messages_have_the_documented_shape() {
        let text = |o: Out| match o {
            Out::Text(t) => serde_json::from_str::<Value>(&t).unwrap(),
            Out::Binary(_) | Out::Ping => panic!("not text"),
        };
        assert_eq!(
            text(data(1, json!([1]))),
            json!({"sub": 1, "type": "data", "data": [1]})
        );
        assert_eq!(
            text(gap(2, 9)),
            json!({"sub": 2, "type": "gap", "dropped": 9})
        );
        assert_eq!(
            text(end(3, "exited", json!({"exit_code": 0}))),
            json!({"sub": 3, "type": "end", "reason": "exited", "exit_code": 0})
        );
        assert_eq!(
            text(error(None, "invalid", "bad frame")),
            json!({"sub": null, "type": "error", "code": "invalid", "message": "bad frame"})
        );
        assert_eq!(
            text(env_status("local", &json!({"state": "down"}))),
            json!({"type": "env_status", "env": "local", "status": {"state": "down"}})
        );
    }

    #[test]
    fn binary_frames_round_trip() {
        let f = encode_binary(0x0102_0304, b"hi");
        assert_eq!(f, [1, 2, 3, 4, b'h', b'i']);
        assert_eq!(decode_binary(&f), Some((0x0102_0304, &b"hi"[..])));
        assert_eq!(decode_binary(&[0, 0, 0, 5]), Some((5, &b""[..])));
        assert_eq!(decode_binary(&[0, 0, 1]), None);
    }
}
