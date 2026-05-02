//! Wire-format frame types for the WebSocket logs stream.
//!
//! Each WS text frame is a JSON object whose `type` discriminates the
//! payload: `hello` (server attached to a pod log stream), `log` (one
//! line of pod stdout/stderr), `error` (recoverable failure surfaced to
//! the client before `end`), and `end` (server is about to close).
//!
//! Variant names are kebab-case on the wire; field names stay
//! `snake_case` so the frontend Zod schemas can mirror Rust struct names
//! directly.

use axum::extract::ws::{Message, Utf8Bytes};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// One frame sent server -> client over `/api/servers/{id}/logs/stream`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Frame {
    /// Server attached to a pod log stream.
    Hello {
        /// Pod name the stream is attached to (e.g. `mc-abcd1234-0`).
        pod: String,
        /// UTC timestamp at attach time.
        attached_at: DateTime<Utc>,
    },
    /// One line of pod stdout/stderr.
    Log {
        /// Trailing `\n` is stripped by the server.
        line: String,
    },
    /// Recoverable or fatal error reported before an upcoming `end`.
    Error {
        /// Stable kebab-case identifier the client can branch on.
        code: &'static str,
        /// Human-readable detail.
        message: String,
    },
    /// Final frame; server is about to close the WebSocket.
    End {
        /// Why the stream is ending.
        reason: EndReason,
    },
}

/// Why the server is closing the WebSocket.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EndReason {
    /// The pod has been unavailable for longer than the wait window
    /// (default 60s). Frontend backoff loop should retry.
    PodUnavailable,
    /// Client sent a Close frame; server tearing down cleanly.
    ClientClosed,
    /// The handler's outer task is exiting (process shutdown / panic).
    ServerShutdown,
}

impl Frame {
    /// Serializes the frame and wraps it in an axum WebSocket text
    /// `Message`. Infallible for the concrete fields used here — a
    /// failure would mean a programming bug (per
    /// [`M-PANIC-ON-BUG`](ms-rust)).
    ///
    /// # Panics
    ///
    /// If `serde_json::to_string` fails. This is unreachable for the
    /// fields the enum actually holds (`String`, `&'static str`,
    /// `DateTime<Utc>`, `EndReason`).
    #[must_use]
    pub fn into_message(self) -> Message {
        let payload =
            serde_json::to_string(&self).expect("Frame serialization is infallible for our types");
        Message::Text(Utf8Bytes::from(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn hello_frame_uses_kebab_type_and_snake_fields() {
        let ts = Utc.with_ymd_and_hms(2026, 5, 2, 12, 0, 0).unwrap();
        let f = Frame::Hello {
            pod: "mc-abc-0".to_owned(),
            attached_at: ts,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(
            json,
            r#"{"type":"hello","pod":"mc-abc-0","attached_at":"2026-05-02T12:00:00Z"}"#
        );
    }

    #[test]
    fn log_frame_carries_line_verbatim() {
        let f = Frame::Log {
            line: "[Server thread/INFO]: Done (1.234s)!".to_owned(),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(
            json,
            r#"{"type":"log","line":"[Server thread/INFO]: Done (1.234s)!"}"#
        );
    }

    #[test]
    fn error_frame_includes_code_and_message() {
        let f = Frame::Error {
            code: "pod-not-found",
            message: "pod is gone".to_owned(),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(
            json,
            r#"{"type":"error","code":"pod-not-found","message":"pod is gone"}"#
        );
    }

    #[test]
    fn end_frame_reasons_render_kebab_case() {
        let cases = [
            (EndReason::PodUnavailable, "pod-unavailable"),
            (EndReason::ClientClosed, "client-closed"),
            (EndReason::ServerShutdown, "server-shutdown"),
        ];
        for (reason, expected) in cases {
            let json = serde_json::to_string(&Frame::End { reason }).unwrap();
            assert_eq!(json, format!(r#"{{"type":"end","reason":"{expected}"}}"#));
        }
    }

    #[test]
    fn into_message_yields_text_with_serialized_payload() {
        let msg = Frame::Log {
            line: "hi".to_owned(),
        }
        .into_message();
        match msg {
            Message::Text(t) => {
                assert_eq!(t.as_str(), r#"{"type":"log","line":"hi"}"#);
            }
            other => panic!("expected Text, got: {other:?}"),
        }
    }
}
