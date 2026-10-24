//! What a figure is, as plain data. Nothing here knows about clocks or
//! registers: the catalog builds these from library values, the renderers draw
//! them, and the two never meet.
//!
//! The collections are private and filled only through the builder methods,
//! which check their arguments, so a renderer can rely on every message going
//! forward in time between two different lanes.

/// One actor's timeline in a [`SpaceTime`] diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneId(pub(crate) usize);

/// One recorded event. Only [`SpaceTime::event`] hands these out, so a message
/// can only ever join two events that were actually recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventId(pub(crate) usize);

#[derive(Debug, Clone)]
pub struct Lane {
    pub name: String,
}

/// Something that happened at one actor. `annotation` is the library value
/// after the event, as text (see [`Label`](crate::label::Label)); it may span
/// several lines.
#[derive(Debug, Clone)]
pub struct Event {
    pub lane: LaneId,
    pub name: String,
    pub annotation: String,
}

/// Information flowing from one event to a later one on another lane: a
/// message, a sync, a copy. An empty label draws a bare arrow.
#[derive(Debug, Clone)]
pub struct Message {
    pub from: EventId,
    pub to: EventId,
    pub label: String,
}

/// A remark about two events, which the renderer highlights. `text` is complete
/// prose: the catalog has the values in hand and writes the sentence.
#[derive(Debug, Clone)]
pub struct Callout {
    pub a: EventId,
    pub b: EventId,
    pub text: String,
}

/// Actors as horizontal lanes, time flowing left to right. The order events
/// were recorded in *is* the time axis.
#[derive(Debug, Clone)]
pub struct SpaceTime {
    title: String,
    lanes: Vec<Lane>,
    events: Vec<Event>,
    messages: Vec<Message>,
    callouts: Vec<Callout>,
}

impl SpaceTime {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lanes: Vec::new(),
            events: Vec::new(),
            messages: Vec::new(),
            callouts: Vec::new(),
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn lanes(&self) -> &[Lane] {
        &self.lanes
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn callouts(&self) -> &[Callout] {
        &self.callouts
    }

    pub fn lane(&mut self, name: impl Into<String>) -> LaneId {
        self.lanes.push(Lane { name: name.into() });
        LaneId(self.lanes.len() - 1)
    }

    pub fn event(
        &mut self,
        lane: LaneId,
        name: impl Into<String>,
        annotation: impl Into<String>,
    ) -> EventId {
        assert!(lane.0 < self.lanes.len(), "unknown lane");
        self.events.push(Event {
            lane,
            name: name.into(),
            annotation: annotation.into(),
        });
        EventId(self.events.len() - 1)
    }

    /// Panics unless `from` was recorded before `to` and on a different lane: a
    /// message always goes forward in time and between actors.
    pub fn message(&mut self, from: EventId, to: EventId, label: impl Into<String>) {
        assert!(to.0 < self.events.len(), "unknown event");
        assert!(from.0 < to.0, "a message must arrive after it was sent");
        assert_ne!(
            self.events[from.0].lane, self.events[to.0].lane,
            "a message must go to another lane"
        );
        self.messages.push(Message {
            from,
            to,
            label: label.into(),
        });
    }

    pub fn callout(&mut self, a: EventId, b: EventId, text: impl Into<String>) {
        assert!(
            a.0 < self.events.len() && b.0 < self.events.len(),
            "unknown event"
        );
        self.callouts.push(Callout {
            a,
            b,
            text: text.into(),
        });
    }

    pub fn lane_of(&self, event: EventId) -> LaneId {
        self.events[event.0].lane
    }
}

/// One actor's counters on a number line, marking which dots were actually
/// seen, against what one or more representations *claim* to contain.
#[derive(Debug, Clone)]
pub struct DotLine {
    title: String,
    actor: String,
    counters: u64,
    present: Vec<u64>,
    claims: Vec<Claim>,
}

/// What one representation says it contains. A counter claimed here but absent
/// from the present set is the mistake the figure is about.
#[derive(Debug, Clone)]
pub struct Claim {
    pub title: String,
    /// The representation itself, as text.
    pub text: String,
    pub counters: Vec<u64>,
}

impl DotLine {
    /// The line shows `1..=counters` for `actor`; `present` are the counters
    /// whose dot really was seen. Panics unless every present or claimed
    /// counter is on the line, and the line is short enough to be a figure.
    pub fn new(
        title: impl Into<String>,
        actor: impl Into<String>,
        counters: u64,
        present: Vec<u64>,
        claims: Vec<Claim>,
    ) -> Self {
        assert!(
            (1..=64).contains(&counters),
            "a number line needs 1 to 64 counters"
        );
        let on_line = |c: &u64| (1..=counters).contains(c);
        assert!(
            present.iter().all(on_line),
            "a present counter is off the line"
        );
        assert!(
            claims.iter().all(|c| c.counters.iter().all(on_line)),
            "a claimed counter is off the line"
        );
        Self {
            title: title.into(),
            actor: actor.into(),
            counters,
            present,
            claims,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    pub fn counters(&self) -> u64 {
        self.counters
    }

    pub fn present(&self) -> &[u64] {
        &self.present
    }

    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }
}

/// Every kind of figure a backend may be asked to draw.
#[derive(Debug, Clone)]
pub enum Diagram {
    SpaceTime(SpaceTime),
    DotLine(DotLine),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_index_in_recorded_order() {
        let mut d = SpaceTime::new("t");
        let a = d.lane("A");
        let e0 = d.event(a, "x", "");
        let e1 = d.event(a, "y", "");
        assert_eq!((e0, e1), (EventId(0), EventId(1)));
        assert_eq!(d.lane_of(e1), a);
    }

    #[test]
    #[should_panic(expected = "arrive after")]
    fn a_message_cannot_go_backwards() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "", "");
        let b1 = d.event(b, "", "");
        d.message(b1, a1, "");
    }

    #[test]
    #[should_panic(expected = "another lane")]
    fn a_message_cannot_stay_on_one_lane() {
        let mut d = SpaceTime::new("t");
        let a = d.lane("A");
        let a1 = d.event(a, "", "");
        let a2 = d.event(a, "", "");
        d.message(a1, a2, "");
    }

    #[test]
    #[should_panic(expected = "unknown event")]
    fn a_callout_needs_recorded_events() {
        let mut d = SpaceTime::new("t");
        let a = d.lane("A");
        let a1 = d.event(a, "", "");
        d.callout(a1, EventId(7), "");
    }

    #[test]
    #[should_panic(expected = "off the line")]
    fn a_dot_line_rejects_counters_it_cannot_draw() {
        DotLine::new("t", "A", 3, vec![0], vec![]);
    }
}
