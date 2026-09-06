//! What the debug surface (K11)
//! remember, and the retention rules that keep either from growing
//! without bound. Pure: the clock arrives as a parameter rather than
//! being read here (AR13), so retention is testable without waiting.
//!
//! Both are deliberately in-memory and lossy. They exist to answer
//! "what just happened" while debugging, not to be a record — the
//! journal and Google hold everything that matters durably (AR16).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// One processed event's route, for K11: what arrived, which profile
/// handled it, and what became of it at Google.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteRecord {
    pub at: String,
    pub source_id: String,
    pub entry_id: String,
    /// The upsert key the delivery resolved to, when it had one.
    pub upsert_key: Option<String>,
    pub outcome: RouteOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum RouteOutcome {
    Created {
        event_id: String,
    },
    Updated {
        event_id: String,
    },
    /// Carries the remedy as well as the message: a debug surface that
    /// shows what broke without saying what to do about it is half a
    /// tool (standing rule 11).
    Failed {
        message: String,
        remedy: String,
    },
}

/// A bounded, newest-first history. Oldest entries fall off the end
/// once `capacity` is reached.
#[derive(Debug)]
pub struct RingBuffer<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            items: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn push(&mut self, item: T) {
        self.items.push_front(item);
        while self.items.len() > self.capacity {
            self.items.pop_back();
        }
    }

    /// Newest first.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn retain<F: FnMut(&T) -> bool>(&mut self, predicate: F) {
        self.items.retain(predicate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_outcome_round_trips_through_json() {
        let record = RouteRecord {
            at: "2026-08-28T09:00:00+00:00".to_string(),
            source_id: "home-assistant".to_string(),
            entry_id: "j1".to_string(),
            upsert_key: Some("home-assistant:switch.wasmachine".to_string()),
            outcome: RouteOutcome::Created {
                event_id: "evt123".to_string(),
            },
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: RouteRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn a_failed_outcome_carries_its_remedy() {
        let outcome = RouteOutcome::Failed {
            message: "Calendar API returned HTTP 404".to_string(),
            remedy: "check the event id and calendar id".to_string(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["result"], "failed");
        assert!(json["remedy"].as_str().unwrap().contains("check"));
    }
}
