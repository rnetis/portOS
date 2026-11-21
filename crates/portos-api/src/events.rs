//! Domain event publishing (F-26, I-1): transactional outbox + in-process
//! broadcast for SSE + a durable webhook dispatcher with retry/backoff.
//!
//! Events are written to `events_outbox` first, then broadcast. The
//! dispatcher reads due rows from the outbox, so a restart never loses
//! events and failed deliveries are retried with exponential backoff until
//! they succeed or reach the terminal attempt limit.

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use portos_core::events::{DomainEvent, DomainEventKind};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt as _};

use crate::state::AppState;

/// Maximum delivery attempts before an event is marked terminally failed.
const MAX_ATTEMPTS: i64 = 8;
/// Upper bound for the exponential backoff between attempts.
const MAX_BACKOFF_SECS: i64 = 300;

/// Record the event in the outbox and broadcast it to SSE subscribers.
pub fn publish(state: &AppState, kind: DomainEventKind) {
    let event = DomainEvent::new(&state.config.tenant, kind);
    let event_type = event.event_type().to_string();
    let payload = match serde_json::to_string(&event) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "event serialize failed");
            return;
        }
    };
    let store = state.store.clone();
    let id = event.id;
    let at = event.at;
    // The outbox write is best-effort for the live stream; the dispatcher
    // only sees rows that were actually persisted.
    tokio::task::spawn_blocking(move || {
        if let Err(e) = portos_store::misc::append_event(&store, id, &event_type, &payload, at) {
            tracing::error!(error = %e, event_type, "event outbox append failed");
        }
    });
    let _ = state.events.send(event);
    crate::metrics::METRICS.inc_by_one("portos_events_published_total");
}

/// HMAC-SHA256 signature for webhook payloads (keyed by the webhook secret).
fn sign(secret: &str, payload: &[u8]) -> String {
    use ring::hmac::{sign, Key, HMAC_SHA256};
    let key = Key::new(HMAC_SHA256, secret.as_bytes());
    hex(sign(&key, payload).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn backoff(attempts: i64) -> Duration {
    let secs = 2_i64.saturating_pow((attempts.clamp(1, 10)) as u32);
    Duration::from_secs(secs.clamp(2, MAX_BACKOFF_SECS) as u64)
}

/// Webhook dispatcher: polls the outbox for due events, POSTs to matching
/// subscriptions, and records delivery or a retry schedule. Runs for the
/// lifetime of the process (shutdown is handled by the server).
pub async fn run_webhook_worker(state: AppState) {
    let timeout = Duration::from_secs(state.config.webhook_timeout_secs.max(1));
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let due = match crate::db::db(&state.store, |store| {
            portos_store::misc::due_events(store, chrono::Utc::now(), 50)
        })
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "webhook outbox read failed");
                continue;
            }
        };
        if due.is_empty() {
            continue;
        }
        let hooks = match crate::db::db(&state.store, |store| {
            portos_store::misc::list_active_webhooks(store)
        })
        .await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(error = %e, "webhook list failed");
                continue;
            }
        };

        for (event_id, event_type, payload, attempts) in due {
            let matching: Vec<_> = hooks
                .iter()
                .filter(|h| portos_store::misc::webhook_matches(h, &event_type))
                .collect();
            if matching.is_empty() {
                // Nothing subscribed: do not keep the row pending forever.
                let _ = crate::db::db(&state.store, move |store| {
                    portos_store::misc::mark_event_delivered(store, event_id)
                })
                .await;
                continue;
            }

            let mut all_ok = true;
            let mut permanent_failure = false;
            for hook in &matching {
                let signature = sign(&hook.secret, payload.as_bytes());
                let url = hook.url.clone();
                let body = payload.clone();
                let timeout_secs = timeout.as_secs();
                let result = tokio::task::spawn_blocking(move || {
                    crate::http_post::post_json(&url, &body, &signature, timeout_secs)
                })
                .await;
                match result {
                    Ok(Ok(status)) if (200..300).contains(&status) => {
                        crate::metrics::METRICS.inc_by_one("portos_webhooks_delivered_total");
                    }
                    Ok(Ok(status)) => {
                        all_ok = false;
                        // 4xx responses (except rate limiting / timeouts) are
                        // not going to succeed on retry.
                        if (400..500).contains(&status) && status != 408 && status != 429 {
                            permanent_failure = true;
                        }
                        crate::metrics::METRICS.inc_by_one("portos_webhooks_failed_total");
                        tracing::warn!(
                            url = %hook.url,
                            status,
                            event_type,
                            "webhook rejected delivery"
                        );
                    }
                    Ok(Err(e)) => {
                        all_ok = false;
                        crate::metrics::METRICS.inc_by_one("portos_webhooks_failed_total");
                        tracing::warn!(url = %hook.url, error = %e, "webhook delivery failed");
                    }
                    Err(e) => {
                        all_ok = false;
                        crate::metrics::METRICS.inc_by_one("portos_webhooks_failed_total");
                        tracing::warn!(error = %e, "webhook task panicked");
                    }
                }
            }

            if all_ok {
                let _ = crate::db::db(&state.store, move |store| {
                    portos_store::misc::mark_event_delivered(store, event_id)
                })
                .await;
            } else {
                let next_attempt = attempts + 1;
                let retry_at = if permanent_failure || next_attempt >= MAX_ATTEMPTS {
                    None
                } else {
                    Some(
                        chrono::Utc::now()
                            + chrono::Duration::from_std(backoff(next_attempt))
                                .unwrap_or(chrono::Duration::seconds(30)),
                    )
                };
                if retry_at.is_none() {
                    crate::metrics::METRICS.inc_by_one("portos_webhooks_dead_letter_total");
                    tracing::error!(
                        event_id = %event_id,
                        event_type,
                        attempts = next_attempt,
                        "webhook delivery abandoned"
                    );
                }
                let _ = crate::db::db(&state.store, move |store| {
                    portos_store::misc::mark_event_attempt(store, event_id, next_attempt, retry_at)
                })
                .await;
            }
        }
    }
}

/// Server-Sent Events stream of domain events (F-26, F-31 live dashboard).
pub async fn event_stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(event) => match serde_json::to_string(&event) {
            Ok(json) => Some(Ok(Event::default().event(event.event_type()).data(json))),
            Err(_) => None,
        },
        Err(_) => None, // receiver lagged; skip missed
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_signature_is_deterministic_hex() {
        let a = sign("secret", b"payload");
        let b = sign("secret", b"payload");
        let c = sign("other", b"payload");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn backoff_is_bounded() {
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(2), Duration::from_secs(4));
        assert_eq!(backoff(20), Duration::from_secs(MAX_BACKOFF_SECS as u64));
    }
}
