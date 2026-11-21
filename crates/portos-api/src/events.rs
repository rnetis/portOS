//! Domain event publishing (F-26, I-1): transactional outbox + in-process
//! broadcast for SSE + webhook dispatch worker with HMAC signatures.

use axum::response::sse::{Event, KeepAlive, Sse};
use axum::extract::State;
use portos_core::events::{DomainEvent, DomainEventKind};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt as _};

use crate::auth::WebhookDelivery;
use crate::state::AppState;

/// Queue of events awaiting webhook delivery.
#[derive(Default)]
pub struct WebhookQueue {
    pub pending: Mutex<VecDeque<WebhookDelivery>>,
}

impl WebhookQueue {
    pub fn push(&self, d: WebhookDelivery) {
        if let Ok(mut q) = self.pending.lock() {
            q.push_back(d);
        }
    }

    pub fn pop(&self) -> Option<WebhookDelivery> {
        self.pending.lock().ok().and_then(|mut q| q.pop_front())
    }
}

/// Record the event in the outbox and broadcast it to SSE subscribers.
/// Outbox rows are delivered to webhooks by the dispatcher worker.
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
    let outbox_type = event_type.clone();
    let outbox_payload = payload.clone();
    // Outbox write is best-effort: the SSE stream still works if it fails.
    tokio::task::spawn_blocking(move || {
        if let Err(e) = portos_store::misc::append_event(&store, id, &outbox_type, &outbox_payload, at) {
            tracing::warn!(error = %e, "event outbox append failed");
        }
    });
    let _ = state.events.send(event.clone());
    state.webhooks.push(WebhookDelivery { event_id: event.id, event_type, payload });
    crate::metrics::METRICS.inc_by_one("portos_events_published_total");
}

/// HMAC-SHA256 signature for webhook payloads (keyed by the webhook secret).
fn sign(secret: &str, payload: &[u8]) -> String {
    use ring::hmac::{sign, Key, HMAC_SHA256};
    let key = Key::new(HMAC_SHA256, secret.as_bytes());
    hex(&sign(&key, payload).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Webhook dispatcher: drains the queue, POSTs to matching subscriptions,
/// marks the outbox delivered. Runs for the lifetime of the process.
pub async fn run_webhook_worker(state: AppState) {
    let timeout = Duration::from_secs(state.config.webhook_timeout_secs.max(1));
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tick.tick() => {
                let Some(delivery) = state.webhooks.pop() else { continue };
                let hooks = match crate::db::db(&state.store, |store| {
                    portos_store::misc::list_active_webhooks(store)
                }).await {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::warn!(error = %e, "webhook list failed");
                        continue;
                    }
                };
                for hook in hooks {
                    if !portos_store::misc::webhook_matches(&hook, &delivery.event_type) {
                        continue;
                    }
                    let signature = sign(&hook.secret, delivery.payload.as_bytes());
                    let url = hook.url.clone();
                    let payload = delivery.payload.clone();
                    let timeout_secs = timeout.as_secs() as u64;
                    let result = tokio::task::spawn_blocking(move || {
                        crate::http_post::post_json(&url, &payload, &signature, timeout_secs)
                    }).await;
                    match result {
                        Ok(Ok(_)) => {
                            crate::metrics::METRICS.inc_by_one("portos_webhooks_delivered_total");
                            let event_id = delivery.event_id;
                            let _ = crate::db::db(&state.store, move |store| {
                                portos_store::misc::mark_event_delivered(store, event_id)
                            }).await;
                        }
                        Ok(Err(e)) => {
                            crate::metrics::METRICS.inc_by_one("portos_webhooks_failed_total");
                            tracing::warn!(url = %hook.url, error = %e, "webhook delivery failed");
                            let event_id = delivery.event_id;
                            let _ = crate::db::db(&state.store, move |store| {
                                portos_store::misc::bump_event_attempts(store, event_id)
                            }).await;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "webhook task panicked");
                        }
                    }
                }
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
            Ok(json) => Some(Ok(Event::default()
                .event(event.event_type())
                .data(json))),
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
}
