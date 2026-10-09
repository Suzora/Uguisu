//! The in-process event bus (ADR 0010, ADR 0016).
//!
//! Events are appended to the `events` table inside the transaction that
//! produced them and published on the bus only after that transaction has
//! committed, so a subscriber never sees an event whose state did not
//! make it to disk. The bus is a bounded broadcast channel: a slow
//! subscriber loses old events (and is told so) rather than blocking the
//! engine.

use tokio::sync::broadcast;
use uguisu_core::Event;
use uguisu_download::EventSink;

/// Default channel capacity per subscriber.
pub const DEFAULT_CAPACITY: usize = 1024;

/// Publishes committed domain events to live subscribers.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl EventBus {
    /// A bus whose subscribers buffer up to `capacity` events each.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// Subscribes to every event published from now on.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        Subscription {
            rx: self.tx.subscribe(),
        }
    }

    /// Publishes committed events in order. Returns how many subscribers
    /// received them (0 when nobody listens; that is not an error).
    pub fn publish(&self, events: &[Event]) -> usize {
        let mut delivered = 0;
        for event in events {
            match self.tx.send(event.clone()) {
                Ok(n) => delivered = n,
                Err(_) => delivered = 0,
            }
            tracing::trace!(kind = event.name(), id = %event.id, "event published");
        }
        delivered
    }

    /// Number of live subscribers.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl EventSink for EventBus {
    fn publish(&self, events: &[Event]) {
        Self::publish(self, events);
    }
}

/// A subscriber's end of the bus.
#[derive(Debug)]
pub struct Subscription {
    rx: broadcast::Receiver<Event>,
}

impl Subscription {
    /// The next event, or `None` once the bus is gone. A subscriber that
    /// fell behind skips the lost events with a warning and continues.
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(lost = n, "event subscriber lagged; events skipped");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Non-blocking variant: the next buffered event, if any.
    pub fn try_recv(&mut self) -> Option<Event> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!(lost = n, "event subscriber lagged; events skipped");
                }
                Err(_) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use uguisu_core::events::EventKind;

    use super::*;

    fn event(i: u32) -> Event {
        Event::now(
            None,
            None,
            EventKind::PodcastMetadataUpdated {
                fields: vec![i.to_string()],
            },
        )
    }

    #[tokio::test]
    async fn delivers_in_order_and_reports_subscribers() {
        let bus = EventBus::new(8);
        assert_eq!(bus.publish(&[event(0)]), 0, "nobody listens yet");
        let mut sub = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        let events = [event(1), event(2)];
        assert_eq!(bus.publish(&events), 1);
        assert_eq!(sub.recv().await.unwrap(), events[0]);
        assert_eq!(sub.try_recv().unwrap(), events[1]);
        assert!(sub.try_recv().is_none());
    }

    #[tokio::test]
    async fn lagging_subscriber_skips_and_continues() {
        let bus = EventBus::new(2);
        let mut sub = bus.subscribe();
        let events: Vec<Event> = (0..5).map(event).collect();
        bus.publish(&events);
        // Capacity 2: the two newest survive, the rest are reported as lost.
        assert_eq!(sub.recv().await.unwrap(), events[3]);
        assert_eq!(sub.recv().await.unwrap(), events[4]);
        drop(bus);
        assert!(sub.recv().await.is_none());
    }
}
