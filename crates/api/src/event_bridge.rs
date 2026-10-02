//! Relaying domain events between server instances.
//!
//! The event bus is a process-local broadcast channel. A second server
//! behind a load balancer never saw the first's events, so publishing a
//! post left the other instance's render cache serving the old version
//! indefinitely — the single reason this could not run on more than one
//! machine.
//!
//! Postgres `LISTEN`/`NOTIFY` carries them across. The database is already
//! the one thing every instance shares, so this adds no new dependency:
//! an event raised locally is written to a channel, and every instance —
//! including the one that raised it — hears it back.
//!
//! Media files and the search index are still node-local. Multi-instance
//! is correct for *content* with this in place; the remaining two need
//! object storage and a shared index respectively.

use sqlx::postgres::PgListener;
use sqlx::PgPool;
use vyasa_core::events::{self, Event};

/// The Postgres channel name. `NOTIFY` identifiers are lowercase.
const CHANNEL: &str = "vyasa_events";

/// Longest a payload may be. Postgres refuses anything over 8000 bytes,
/// and every event we send is a couple of ids.
const MAX_PAYLOAD: usize = 7000;

/// Starts the bridge: relays local events out, and applies remote ones.
///
/// A no-op when the instance id cannot be generated. Failures are logged
/// and never fatal — a site with a broken bridge is a single-instance site,
/// which is what it was before.
pub fn spawn(pool: &PgPool) {
    // Each instance tags what it sends so it can ignore the echo. Without
    // it every event would be delivered twice locally: once by `emit` and
    // once by the listener.
    let instance = vyasa_common::next_id_i64();

    let outbound = pool.clone();
    let relay = move |event: &Event| {
        let Ok(payload) = serde_json::to_string(&Envelope {
            instance,
            event: event.clone(),
        }) else {
            return;
        };
        if payload.len() > MAX_PAYLOAD {
            tracing::warn!("event too large to relay ({} bytes)", payload.len());
            return;
        }
        let pool = outbound.clone();
        tokio::spawn(async move {
            // `pg_notify` rather than a literal `NOTIFY`, so the payload is
            // a bound parameter instead of something spliced into SQL.
            if let Err(e) = sqlx::query("SELECT pg_notify($1, $2)")
                .bind(CHANNEL)
                .bind(&payload)
                .execute(&pool)
                .await
            {
                tracing::warn!("could not relay event: {e}");
            }
        });
    };
    if events::set_relay(Box::new(relay)).is_err() {
        tracing::warn!("event relay already installed");
        return;
    }

    let inbound = pool.clone();
    tokio::spawn(async move {
        loop {
            match listen(&inbound, instance).await {
                // The listener only returns on error; a clean end means the
                // pool closed, which happens at shutdown.
                // The pool closes at shutdown; that is the end, not an
                // outage to retry and warn about while the process exits.
                Ok(()) | Err(sqlx::Error::PoolClosed) => break,
                Err(e) if e.to_string().contains("closed pool") => break,
                Err(e) => {
                    tracing::warn!("event listener stopped: {e}; retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}

/// One instance's view of an event on the wire.
#[derive(serde::Serialize, serde::Deserialize)]
struct Envelope {
    instance: i64,
    event: Event,
}

async fn listen(pool: &PgPool, instance: i64) -> Result<(), sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(CHANNEL).await?;
    tracing::info!("listening for events from other instances");
    loop {
        let notification = listener.recv().await?;
        let Ok(envelope) = serde_json::from_str::<Envelope>(notification.payload()) else {
            tracing::warn!("ignoring an unreadable event from the bus");
            continue;
        };
        // Our own echo: `emit` already delivered it locally.
        if envelope.instance == instance {
            continue;
        }
        events::emit_remote(envelope.event);
    }
}

#[cfg(test)]
mod tests {
    use super::{Envelope, MAX_PAYLOAD};
    use vyasa_core::events::{Event, PostPublished};

    #[test]
    fn an_envelope_round_trips_and_stays_small_enough_to_notify() {
        let envelope = Envelope {
            instance: 7,
            event: Event::Published(PostPublished {
                post_id: 42,
                author_id: 1,
            }),
        };
        let wire = serde_json::to_string(&envelope).expect("serializes");
        assert!(
            wire.len() < MAX_PAYLOAD,
            "an event must fit in a NOTIFY payload: {} bytes",
            wire.len()
        );
        let back: Envelope = serde_json::from_str(&wire).expect("round trips");
        assert_eq!(back.instance, 7);
        match back.event {
            Event::Published(p) => assert_eq!(p.post_id, 42),
            other => panic!("wrong event: {other:?}"),
        }
    }
}
