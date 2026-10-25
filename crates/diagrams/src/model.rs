//! What a figure is, as plain data. Nothing here knows about clocks or
//! registers: the catalog builds these from library values, the renderers draw
//! them, and the two never meet.
//!
//! The collections are private and filled only through the builder methods,
//! which check their arguments, so a renderer can rely on every message going
//! forward in time between two different lanes.

use std::collections::BTreeSet;

/// One actor's timeline in a [`SpaceTime`] diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneId(pub(crate) usize);

/// One recorded event. Only [`SpaceTime::event`] hands these out, so a message
/// can only ever join two events that were actually recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventId(pub(crate) usize);

/// A run of text and what it is. Renderers turn each role into a `ds-<role>` class
/// and each `data` pair into a `data-<key>` attribute, so a page can style, say,
/// one actor's clock entries or one dot wherever they appear. `value` marks text
/// that came from a library value, which is set in monospace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub roles: Vec<&'static str>,
    pub data: Vec<(&'static str, String)>,
    pub value: bool,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Span {
        Span {
            text: text.into(),
            roles: Vec::new(),
            data: Vec::new(),
            value: false,
        }
    }

    /// Part of a library value, with no role of its own (punctuation, say).
    pub fn value(text: impl Into<String>) -> Span {
        Span {
            value: true,
            ..Span::plain(text)
        }
    }

    /// Part of a library value with a role, such as `entry` or `dot-id`.
    pub fn part(text: impl Into<String>, role: &'static str) -> Span {
        Span {
            roles: vec![role],
            ..Span::value(text)
        }
    }

    /// One more role, such as `cloud` on a dot past a causal context's gap.
    pub fn also(mut self, role: &'static str) -> Span {
        self.roles.push(role);
        self
    }

    /// Sets `data-<key>`, replacing any earlier value for the same key.
    pub fn with(mut self, key: &'static str, value: impl Into<String>) -> Span {
        self.data.retain(|(k, _)| *k != key);
        self.data.push((key, value.into()));
        self
    }
}

/// Text as lines of [`Span`]s. A plain string converts with one line per `\n`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rich {
    pub lines: Vec<Vec<Span>>,
}

impl Rich {
    pub fn is_empty(&self) -> bool {
        self.lines
            .iter()
            .all(|l| l.iter().all(|s| s.text.is_empty()))
    }

    /// The text alone, lines joined by `\n`.
    pub fn plain(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// This text with role `changed` on each part (a span with a role or
    /// data) that `before` lacks or reads differently, so a page can show what
    /// an event changed. Lines are matched by the dot they start with, else
    /// their field name, else their position; parts within a line by roles,
    /// data and how many alike parts precede them. Punctuation and symbols are
    /// never marked.
    pub fn mark_changes(&self, before: &Rich) -> Rich {
        fn key(line: &[Span]) -> Option<String> {
            let dot = line
                .first()
                .and_then(|s| s.data.iter().find(|(k, _)| *k == "dot").map(|(_, v)| v));
            match dot {
                Some(d) => Some(format!("dot {d}")),
                None => line
                    .iter()
                    .find(|s| s.roles.contains(&"field"))
                    .map(|s| format!("field {}", s.text.trim())),
            }
        }
        let is_part = |s: &Span| !s.roles.is_empty() && !s.roles.contains(&"sym");
        let mut out = self.clone();
        for (k, line) in out.lines.iter_mut().enumerate() {
            let was: Option<&Vec<Span>> = match key(line) {
                Some(id) => before.lines.iter().find(|l| key(l).as_ref() == Some(&id)),
                None => before.lines.get(k).filter(|l| key(l).is_none()),
            };
            let alike = |a: &Span, b: &Span| a.roles == b.roles && a.data == b.data;
            let changed: Vec<usize> = (0..line.len())
                .filter(|&j| is_part(&line[j]))
                .filter(|&j| {
                    let s = &line[j];
                    let nth = line[..j].iter().filter(|o| alike(o, s)).count();
                    let old =
                        was.and_then(|l| l.iter().filter(|o| is_part(o) && alike(o, s)).nth(nth));
                    // Without padding, which follows the longest field name.
                    old.is_none_or(|o| o.text.trim() != s.text.trim())
                })
                .collect();
            for j in changed {
                line[j].roles.push("changed");
            }
        }
        out
    }

    /// `other` continues this text: its first line joins the last line here,
    /// and any further lines follow.
    pub fn then(mut self, other: impl Into<Rich>) -> Rich {
        let mut more = other.into().lines.into_iter();
        if let Some(first) = more.next() {
            match self.lines.last_mut() {
                Some(last) => last.extend(first),
                None => self.lines.push(first),
            }
        }
        self.lines.extend(more);
        self
    }
}

impl From<&str> for Rich {
    fn from(s: &str) -> Rich {
        if s.is_empty() {
            return Rich::default();
        }
        Rich {
            lines: s
                .split('\n')
                .map(|l| {
                    if l.is_empty() {
                        Vec::new()
                    } else {
                        vec![Span::plain(l)]
                    }
                })
                .collect(),
        }
    }
}

impl From<String> for Rich {
    fn from(s: String) -> Rich {
        Rich::from(s.as_str())
    }
}

impl From<&String> for Rich {
    fn from(s: &String) -> Rich {
        Rich::from(s.as_str())
    }
}

impl From<Span> for Rich {
    fn from(s: Span) -> Rich {
        Rich {
            lines: vec![vec![s]],
        }
    }
}

/// Concatenate anything that converts into [`Rich`]: `rich!["A1 ", v(&clock), "."]`.
macro_rules! rich {
    ($($part:expr),* $(,)?) => {
        $crate::model::Rich::default()$(.then($part))*
    };
}
pub(crate) use rich;

#[derive(Debug, Clone)]
pub struct Lane {
    pub name: String,
    /// Its address on a page; see [`SpaceTime::lane_key`].
    pub key: String,
    /// The actor whose events this lane shows, when it is one: the name that
    /// appears in clock entries and dots. Set by [`SpaceTime::actor_lane`].
    pub actor: Option<String>,
    /// What the lane holds before its first event, for a step-through's
    /// state panel. Set by [`SpaceTime::start_value`].
    pub start: Rich,
}

/// Something that happened at one actor. `annotation` is the library value
/// after the event (see [`Label`](crate::label::Label)); it may span several
/// lines.
#[derive(Debug, Clone)]
pub struct Event {
    pub lane: LaneId,
    pub name: String,
    pub annotation: Rich,
    /// Set by [`SpaceTime::pin_event`]; otherwise the key is positional.
    pub key: Option<String>,
    /// What a step-through says at this event, if the catalog wrote it. Set
    /// by [`SpaceTime::narrate`].
    pub narration: Rich,
}

/// Information flowing from one event to a later one on another lane: a
/// message, a sync, a copy. An empty label draws a bare arrow.
#[derive(Debug, Clone)]
pub struct Message {
    pub from: EventId,
    pub to: EventId,
    pub label: Rich,
    /// Set by [`SpaceTime::pin_message`]; otherwise the key is positional.
    pub key: Option<String>,
}

/// One recorded message, as handed out by [`SpaceTime::message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageId(pub(crate) usize);

/// How a callout's two events are related in the drawn graph, when the callout
/// is about that relation. The builder checks it against the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// `a` happened before `b`: some chain of lane steps and messages joins them.
    Before,
    /// Neither event can reach the other.
    Concurrent,
}

/// A remark about two events, which the renderer highlights. `text` is complete
/// prose: the catalog has the values in hand and writes the sentence.
#[derive(Debug, Clone)]
pub struct Callout {
    pub a: EventId,
    pub b: EventId,
    pub text: Rich,
    pub relation: Option<Relation>,
}

/// A callout with its number and everything it is about.
#[derive(Debug, Clone)]
pub struct Note<'a> {
    /// 1-based, in reading order.
    pub n: usize,
    pub callout: &'a Callout,
    /// The two events, plus for a [`Relation::Before`] every event on a path
    /// between them.
    pub events: BTreeSet<EventId>,
    /// Indices into [`SpaceTime::messages`] of the messages on such a path, or
    /// for any other callout, the messages joining its two events directly.
    pub messages: BTreeSet<usize>,
}

/// What a lane holds at one step; see [`SpaceTime::lane_state`].
#[derive(Debug, Clone, Copy)]
pub struct LaneState<'a> {
    /// The event whose value it is, or `None` for the lane's start value.
    pub event: Option<EventId>,
    pub value: &'a Rich,
    /// What the lane held before that event, if anything.
    pub before: Option<&'a Rich>,
}

/// One step of a step-through figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    /// The lanes, before anything has happened.
    Start,
    Event(EventId),
    /// A note, by its number in [`SpaceTime::notes`].
    Note(usize),
    /// The finished figure.
    End,
}

/// The order a step-through shows a figure in: the lanes, then each event in
/// recorded order, each note right after its later event, then the finished
/// figure. One change per step, and the notes come in the order they are
/// numbered.
#[derive(Debug, Clone)]
pub struct Timeline {
    beats: Vec<Beat>,
    event_step: Vec<usize>,
    /// Indexed by note number less one.
    note_step: Vec<usize>,
}

impl Timeline {
    pub fn beats(&self) -> &[Beat] {
        &self.beats
    }

    /// The step that shows the finished figure.
    pub fn last(&self) -> usize {
        self.beats.len() - 1
    }

    /// The step at which `event` happens.
    pub fn event(&self, event: EventId) -> usize {
        self.event_step[event.0]
    }

    /// The step at which note `n` (from 1) is made.
    pub fn note(&self, n: usize) -> usize {
        self.note_step[n - 1]
    }

    /// The latest event to have happened by `step`, if any: the playhead's
    /// column.
    pub fn now(&self, step: usize) -> Option<EventId> {
        self.beats[..=step].iter().rev().find_map(|b| match b {
            Beat::Event(e) => Some(*e),
            _ => None,
        })
    }
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
    step_through: bool,
}

impl SpaceTime {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lanes: Vec::new(),
            events: Vec::new(),
            messages: Vec::new(),
            callouts: Vec::new(),
            step_through: false,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Mark this figure as one a page steps through, which draws each message
    /// in flight, so its layout leaves room for that. The catalog marks every
    /// `Form::Steps` figure.
    pub fn step_through(&mut self) {
        self.step_through = true;
    }

    /// Is this figure stepped through? See [`SpaceTime::step_through`].
    pub fn is_step_through(&self) -> bool {
        self.step_through
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
        let name = name.into();
        let base = slug(&name);
        let key = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|k| self.lanes.iter().all(|l| &l.key != k))
            .expect("some suffix is free");
        self.lanes.push(Lane {
            name,
            key,
            actor: None,
            start: Rich::default(),
        });
        LaneId(self.lanes.len() - 1)
    }

    /// A lane that shows one actor's events. Its name may differ from the
    /// actor's (replica `r1` writing as actor `A`); the figure links the two, so
    /// a page can colour the lane and that actor's entries in every value alike.
    pub fn actor_lane(&mut self, name: impl Into<String>, actor: impl std::fmt::Display) -> LaneId {
        let id = self.lane(name);
        self.lanes[id.0].actor = Some(actor.to_string());
        id
    }

    /// Every actor the figure mentions, in palette order: the lanes' actors top
    /// to bottom, then any other actor in the order its first value appears.
    /// A renderer gives actor `i` (from 0) the palette slot `i + 1`.
    pub fn actors(&self) -> Vec<String> {
        let mut out: Vec<String> = self.lanes.iter().filter_map(|l| l.actor.clone()).collect();
        let texts = self
            .events
            .iter()
            .map(|e| &e.annotation)
            .chain(self.messages.iter().map(|m| &m.label))
            .chain(self.callouts.iter().map(|c| &c.text))
            .chain(self.lanes.iter().map(|l| &l.start))
            .chain(self.events.iter().map(|e| &e.narration));
        for span in texts.flat_map(|r| r.lines.iter().flatten()) {
            for (k, v) in &span.data {
                if *k == "actor" && !out.contains(v) {
                    out.push(v.clone());
                }
            }
        }
        out
    }

    /// Apply `f` to every span of every text in the figure.
    pub fn for_each_span(&mut self, mut f: impl FnMut(&mut Span)) {
        let texts = self
            .events
            .iter_mut()
            .map(|e| &mut e.annotation)
            .chain(self.messages.iter_mut().map(|m| &mut m.label))
            .chain(self.callouts.iter_mut().map(|c| &mut c.text))
            .chain(self.lanes.iter_mut().map(|l| &mut l.start));
        for r in texts {
            r.lines.iter_mut().flatten().for_each(&mut f);
        }
        // Last, as in `actors`: an event's own text is borrowed above.
        for e in &mut self.events {
            e.narration.lines.iter_mut().flatten().for_each(&mut f);
        }
    }

    pub fn event(
        &mut self,
        lane: LaneId,
        name: impl Into<String>,
        annotation: impl Into<Rich>,
    ) -> EventId {
        assert!(lane.0 < self.lanes.len(), "unknown lane");
        self.events.push(Event {
            lane,
            name: name.into(),
            annotation: annotation.into(),
            key: None,
            narration: Rich::default(),
        });
        EventId(self.events.len() - 1)
    }

    /// What a step-through says at `event`'s step, in place of the account
    /// it would generate. It continues "B2 on B …": say what the library did
    /// and show values with [`v`](crate::label::v), which a test checks are
    /// on screen at that step; any further claim needs an assert beside it in
    /// the catalog, as a callout's does. Panics if the event is unknown, the
    /// text empty, or the event already narrated.
    pub fn narrate(&mut self, event: EventId, text: impl Into<Rich>) {
        assert!(event.0 < self.events.len(), "unknown event");
        let text = text.into();
        assert!(!text.is_empty(), "a narration says something");
        let e = &mut self.events[event.0];
        assert!(e.narration.is_empty(), "the event is already narrated");
        e.narration = text;
    }

    /// Give an event a key that survives changes to the scenario around it.
    /// A post that styles or links one particular event should target a pinned
    /// key: the positional ones move when an earlier event is added. Panics if
    /// the key is not a slug or is already taken.
    pub fn pin_event(&mut self, event: EventId, key: &str) {
        assert!(event.0 < self.events.len(), "unknown event");
        let others = (0..self.events.len()).filter(|&i| i != event.0);
        self.check_new_key(key, others.map(|i| self.event_key(EventId(i))));
        self.events[event.0].key = Some(key.to_string());
    }

    /// Give a message a key that survives changes to the scenario; see
    /// [`SpaceTime::pin_event`].
    pub fn pin_message(&mut self, message: MessageId, key: &str) {
        assert!(message.0 < self.messages.len(), "unknown message");
        let others = (0..self.messages.len()).filter(|&i| i != message.0);
        self.check_new_key(key, others.map(|i| self.message_key(i)));
        self.messages[message.0].key = Some(key.to_string());
    }

    fn check_new_key(&self, key: &str, taken: impl Iterator<Item = String>) {
        assert_eq!(
            slug(key),
            key,
            "a key must be lowercase letters, digits and dashes"
        );
        assert!(
            taken.into_iter().all(|k| k != key),
            "the key {key:?} is taken"
        );
    }

    /// Panics unless `from` was recorded before `to` and on a different lane: a
    /// message always goes forward in time and between actors.
    pub fn message(&mut self, from: EventId, to: EventId, label: impl Into<Rich>) -> MessageId {
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
            key: None,
        });
        MessageId(self.messages.len() - 1)
    }

    pub fn callout(&mut self, a: EventId, b: EventId, text: impl Into<Rich>) {
        self.push_callout(a, b, text.into(), None);
    }

    /// A callout claiming `a` happened before `b`. Panics unless the drawn graph
    /// joins them, so the picture can't contradict the sentence.
    pub fn callout_before(&mut self, a: EventId, b: EventId, text: impl Into<Rich>) {
        self.check_events(a, b);
        assert!(
            self.happened_before(a, b),
            "the figure draws no path from the first event to the second"
        );
        self.push_callout(a, b, text.into(), Some(Relation::Before));
    }

    /// A callout claiming `a` and `b` are concurrent. Panics if the drawn graph
    /// joins them either way.
    pub fn callout_concurrent(&mut self, a: EventId, b: EventId, text: impl Into<Rich>) {
        self.check_events(a, b);
        assert!(
            a != b && !self.happened_before(a, b) && !self.happened_before(b, a),
            "the figure draws a path between the two events"
        );
        self.push_callout(a, b, text.into(), Some(Relation::Concurrent));
    }

    fn check_events(&self, a: EventId, b: EventId) {
        assert!(
            a.0 < self.events.len() && b.0 < self.events.len(),
            "unknown event"
        );
    }

    fn push_callout(&mut self, a: EventId, b: EventId, text: Rich, relation: Option<Relation>) {
        self.check_events(a, b);
        self.callouts.push(Callout {
            a,
            b,
            text,
            relation,
        });
    }

    /// What `lane` holds before its first event: in a step-through, its state
    /// panel shows this until the lane's first value. Panics if the lane is
    /// unknown or already has one.
    pub fn start_value(&mut self, lane: LaneId, value: impl Into<Rich>) {
        assert!(lane.0 < self.lanes.len(), "unknown lane");
        assert!(
            self.lanes[lane.0].start.is_empty(),
            "the lane already has a start value"
        );
        self.lanes[lane.0].start = value.into();
    }

    /// What `lane` holds at `step` of `t`: the value after its latest event by
    /// then whose annotation isn't empty, else its start value, else nothing.
    /// It is the value the figure already shows under that event, so a state
    /// panel can't disagree with the drawing.
    pub fn lane_state(&self, t: &Timeline, lane: LaneId, step: usize) -> Option<LaneState<'_>> {
        let start = &self.lanes[lane.0].start;
        let mut held: Vec<(Option<EventId>, &Rich)> = Vec::new();
        if !start.is_empty() {
            held.push((None, start));
        }
        for (i, e) in self.events.iter().enumerate() {
            if e.lane == lane && t.event(EventId(i)) <= step && !e.annotation.is_empty() {
                held.push((Some(EventId(i)), &e.annotation));
            }
        }
        let &(event, value) = held.last()?;
        let before = held.len().checked_sub(2).map(|k| held[k].1);
        Some(LaneState {
            event,
            value,
            before,
        })
    }

    pub fn lane_of(&self, event: EventId) -> LaneId {
        self.events[event.0].lane
    }

    /// Was `a` recorded before `b`? The figure's left-to-right order.
    pub fn recorded_before(&self, a: EventId, b: EventId) -> bool {
        a.0 < b.0
    }

    /// Each event's causal past in the drawn graph: every event that reaches it
    /// through earlier events on a lane and messages. Messages only go forward,
    /// so one pass in recorded order is enough.
    pub fn pasts(&self) -> Vec<BTreeSet<EventId>> {
        let mut past: Vec<BTreeSet<EventId>> = Vec::with_capacity(self.events.len());
        for (i, e) in self.events.iter().enumerate() {
            let lane_prev = (0..i).rev().find(|&p| self.events[p].lane == e.lane);
            let senders = self
                .messages
                .iter()
                .filter(|m| m.to.0 == i)
                .map(|m| m.from.0);
            let mut set = BTreeSet::new();
            for p in lane_prev.into_iter().chain(senders) {
                set.insert(EventId(p));
                set.extend(past[p].iter().copied());
            }
            past.push(set);
        }
        past
    }

    /// Does the drawn graph carry `a` into `b`'s causal past?
    pub fn happened_before(&self, a: EventId, b: EventId) -> bool {
        self.pasts()[b.0].contains(&a)
    }

    /// The callouts numbered in reading order: by their later event, then their
    /// earlier one, then the order they were added. Every backend uses this.
    pub fn notes(&self) -> Vec<Note<'_>> {
        let mut order: Vec<usize> = (0..self.callouts.len()).collect();
        order.sort_by_key(|&j| {
            let c = &self.callouts[j];
            (c.a.max(c.b), c.a.min(c.b), j)
        });
        let pasts = self.pasts();
        order
            .into_iter()
            .enumerate()
            .map(|(k, j)| {
                let c = &self.callouts[j];
                let mut events = BTreeSet::from([c.a, c.b]);
                let mut messages = BTreeSet::new();
                let joins =
                    |m: &Message| (m.from, m.to) == (c.a, c.b) || (m.from, m.to) == (c.b, c.a);
                messages.extend(
                    self.messages
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| joins(m))
                        .map(|(i, _)| i),
                );
                if c.relation == Some(Relation::Before) {
                    let on_path = |e: EventId| {
                        e == c.a
                            || e == c.b
                            || (pasts[e.0].contains(&c.a) && pasts[c.b.0].contains(&e))
                    };
                    events.extend((0..self.events.len()).map(EventId).filter(|&e| on_path(e)));
                    messages.extend(
                        self.messages
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| on_path(m.from) && on_path(m.to))
                            .map(|(i, _)| i),
                    );
                }
                Note {
                    n: k + 1,
                    callout: c,
                    events,
                    messages,
                }
            })
            .collect()
    }

    /// The figure as a step-through: see [`Timeline`].
    pub fn timeline(&self) -> Timeline {
        let notes = self.notes();
        let mut beats = vec![Beat::Start];
        let mut event_step = Vec::with_capacity(self.events.len());
        let mut note_step = vec![0; notes.len()];
        let mut pending = notes.iter().peekable();
        for i in 0..self.events.len() {
            event_step.push(beats.len());
            beats.push(Beat::Event(EventId(i)));
            while let Some(n) = pending.next_if(|n| n.callout.a.max(n.callout.b).0 == i) {
                note_step[n.n - 1] = beats.len();
                beats.push(Beat::Note(n.n));
            }
        }
        beats.push(Beat::End);
        Timeline {
            beats,
            event_step,
            note_step,
        }
    }

    /// A lane's address in the rendered figure, from its name: `"r1"` → `r1`,
    /// `"P₁"` → `p₁`. A name whose key is taken gets a suffix: `r1-2`.
    pub fn lane_key(&self, lane: LaneId) -> String {
        self.lanes[lane.0].key.clone()
    }

    /// An event's address: its pinned key if it has one, else its lane's key
    /// and its position on that lane, from 1. The third event on lane B is `b-3`.
    pub fn event_key(&self, event: EventId) -> String {
        if let Some(k) = &self.events[event.0].key {
            return k.clone();
        }
        let lane = self.events[event.0].lane;
        let ordinal = self.events[..=event.0]
            .iter()
            .filter(|e| e.lane == lane)
            .count();
        format!("{}-{ordinal}", self.lane_key(lane))
    }

    /// A message's address: its pinned key if it has one, else its place in
    /// recorded order from 1: `m1`, `m2`, ….
    pub fn message_key(&self, index: usize) -> String {
        match &self.messages[index].key {
            Some(k) => k.clone(),
            None => format!("m{}", index + 1),
        }
    }
}

/// Lowercase letters and digits (any script), anything else as `-`, trimmed.
/// Used for keys that end up in attribute values a stylesheet will select on.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "x".to_string() } else { out }
}

/// One actor's counters on a number line, marking which dots were actually
/// seen, against what one or more representations *claim* to contain. A
/// finished figure has one frame; a step-through shows its frames in turn.
#[derive(Debug, Clone)]
pub struct DotLine {
    title: String,
    actor: String,
    counters: u64,
    frames: Vec<Frame>,
}

/// One moment of a number line: the counters whose dot has been seen, what
/// each representation claims, and what a step-through says about it.
#[derive(Debug, Clone)]
pub struct Frame {
    pub present: Vec<u64>,
    pub claims: Vec<Claim>,
    pub narration: Rich,
}

/// What one representation says it contains. A counter claimed here but absent
/// from the present set is the mistake the figure is about.
#[derive(Debug, Clone)]
pub struct Claim {
    pub title: String,
    /// The representation itself, as text.
    pub text: Rich,
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
        let mut d = Self::frames(title, actor, counters);
        d.frame(present, claims, Rich::default());
        d
    }

    /// A number line to show frame by frame, as [`DotLine::frame`] adds them.
    pub fn frames(title: impl Into<String>, actor: impl Into<String>, counters: u64) -> Self {
        assert!(
            (1..=64).contains(&counters),
            "a number line needs 1 to 64 counters"
        );
        Self {
            title: title.into(),
            actor: actor.into(),
            counters,
            frames: Vec::new(),
        }
    }

    /// The next frame. Panics unless every present or claimed counter is on
    /// the line, each claim's counters ascend, and the rows are those of the
    /// first frame: a step-through changes what the rows say, not which rows
    /// there are.
    pub fn frame(&mut self, present: Vec<u64>, claims: Vec<Claim>, narration: impl Into<Rich>) {
        let on_line = |c: &u64| (1..=self.counters).contains(c);
        assert!(
            present.iter().all(on_line),
            "a present counter is off the line"
        );
        assert!(
            claims.iter().all(|c| c.counters.iter().all(on_line)),
            "a claimed counter is off the line"
        );
        assert!(
            claims.iter().all(|c| c.counters.is_sorted_by(|a, b| a < b)),
            "a claim's counters ascend"
        );
        if let Some(first) = self.frames.first() {
            assert!(
                first
                    .claims
                    .iter()
                    .map(|c| &c.title)
                    .eq(claims.iter().map(|c| &c.title)),
                "every frame has the same rows"
            );
        }
        self.frames.push(Frame {
            present,
            claims,
            narration: narration.into(),
        });
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

    /// Every frame, in order.
    pub fn all_frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The last frame: the finished figure.
    fn finished(&self) -> &Frame {
        self.frames.last().expect("a number line needs a frame")
    }

    pub fn present(&self) -> &[u64] {
        &self.finished().present
    }

    pub fn claims(&self) -> &[Claim] {
        &self.finished().claims
    }

    /// The step that shows the finished figure: one step per frame, then the
    /// finished figure again, as a space-time step-through ends.
    pub fn last_step(&self) -> usize {
        self.frames.len()
    }

    /// The frame shown at `step`.
    pub fn frame_at(&self, step: usize) -> &Frame {
        &self.frames[step.min(self.frames.len() - 1)]
    }

    /// Apply `f` to every span of every frame's text.
    pub fn for_each_span(&mut self, mut f: impl FnMut(&mut Span)) {
        for frame in &mut self.frames {
            let texts = frame
                .claims
                .iter_mut()
                .map(|c| &mut c.text)
                .chain([&mut frame.narration]);
            for r in texts {
                r.lines.iter_mut().flatten().for_each(&mut f);
            }
        }
    }
}

/// Every kind of figure a backend may be asked to draw.
#[derive(Debug, Clone)]
pub enum Diagram {
    SpaceTime(SpaceTime),
    DotLine(DotLine),
}

impl Diagram {
    pub fn title(&self) -> &str {
        match self {
            Diagram::SpaceTime(d) => d.title(),
            Diagram::DotLine(d) => d.title(),
        }
    }

    /// The kind as it appears in rendered output: `space-time` or `dot-line`.
    pub fn kind(&self) -> &'static str {
        match self {
            Diagram::SpaceTime(_) => "space-time",
            Diagram::DotLine(_) => "dot-line",
        }
    }
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

    fn claim(title: &str, counters: Vec<u64>) -> Claim {
        Claim {
            title: title.into(),
            text: Rich::default(),
            counters,
        }
    }

    #[test]
    #[should_panic(expected = "same rows")]
    fn every_frame_has_the_same_rows() {
        let mut d = DotLine::frames("t", "A", 3);
        d.frame(vec![], vec![claim("x", vec![])], "");
        d.frame(vec![], vec![claim("y", vec![])], "");
    }

    #[test]
    #[should_panic(expected = "ascend")]
    fn claimed_counters_ascend() {
        let mut d = DotLine::frames("t", "A", 3);
        d.frame(vec![], vec![claim("x", vec![2, 1])], "");
    }

    #[test]
    #[should_panic(expected = "needs a frame")]
    fn a_dot_line_needs_a_frame() {
        DotLine::frames("t", "A", 3).present();
    }

    #[test]
    fn a_dot_line_steps_through_its_frames_then_ends() {
        let mut d = DotLine::frames("t", "A", 3);
        d.frame(vec![1], vec![], "one");
        d.frame(vec![1, 3], vec![], "two");
        assert_eq!(d.last_step(), 2);
        assert_eq!(d.frame_at(0).present, [1]);
        assert_eq!(
            d.frame_at(2).present,
            [1, 3],
            "the end shows the last frame"
        );
        assert_eq!(d.present(), [1, 3]);
    }

    #[test]
    fn lane_keys_are_unique_and_keep_any_script() {
        let mut d = SpaceTime::new("t");
        let (r1, r1p, p1) = (d.lane("r1"), d.lane("r1′"), d.lane("P₁"));
        assert_eq!(
            [d.lane_key(r1), d.lane_key(r1p), d.lane_key(p1)],
            ["r1", "r1-2", "p₁"]
        );
    }

    #[test]
    fn actors_come_in_lane_order_then_by_first_mention() {
        let mut d = SpaceTime::new("t");
        let r1 = d.actor_lane("r1", 'B');
        d.actor_lane("r2", 'A');
        d.event(
            r1,
            "",
            rich![
                Span::part("Z:1", "dot-id").with("actor", "Z"),
                Span::part("A:1", "dot-id").with("actor", "A")
            ],
        );
        assert_eq!(d.actors(), ["B", "A", "Z"]);
    }

    #[test]
    fn pinned_keys_survive_an_inserted_event() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        let a1 = d.event(a, "", "");
        d.event(b, "", "");
        let b2 = d.event(b, "", "");
        let m = d.message(a1, b2, "");
        d.pin_event(b2, "reply");
        d.pin_message(m, "request");
        assert_eq!(
            (d.event_key(b2), d.message_key(m.0)),
            ("reply".into(), "request".into())
        );
    }

    #[test]
    #[should_panic(expected = "is taken")]
    fn a_pinned_key_cannot_shadow_another_event() {
        let mut d = SpaceTime::new("t");
        let a = d.lane("A");
        d.event(a, "", "");
        let a2 = d.event(a, "", "");
        d.pin_event(a2, "a-1");
    }

    #[test]
    fn a_plain_note_includes_the_message_joining_its_events() {
        let (mut d, [a1, b1, ..]) = trace();
        d.callout(b1, a1, "about m1");
        assert_eq!(d.notes()[0].messages, BTreeSet::from([0]));
    }

    #[test]
    fn keys_name_lanes_events_and_messages() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("r2′"));
        let a1 = d.event(a, "", "");
        let b1 = d.event(b, "", "");
        let a2 = d.event(a, "", "");
        d.message(a1, b1, "");
        assert_eq!(d.lane_key(b), "r2");
        assert_eq!(
            [d.event_key(a1), d.event_key(b1), d.event_key(a2)],
            ["a-1", "r2-1", "a-2"]
        );
        assert_eq!(d.message_key(0), "m1");
    }

    /// The textbook trace: A1 reaches C2 only through B; A3 is concurrent with B3.
    fn trace() -> (SpaceTime, [EventId; 5]) {
        let mut d = SpaceTime::new("t");
        let (a, b, c) = (d.lane("A"), d.lane("B"), d.lane("C"));
        let a1 = d.event(a, "A1", "");
        let b1 = d.event(b, "B1", "");
        let b2 = d.event(b, "B2", "");
        let c1 = d.event(c, "C1", "");
        let a2 = d.event(a, "A2", "");
        d.message(a1, b1, "");
        d.message(b2, c1, "");
        (d, [a1, b1, b2, c1, a2])
    }

    #[test]
    fn the_drawn_graph_follows_lanes_and_messages() {
        let (d, [a1, b1, b2, c1, a2]) = trace();
        assert!(d.happened_before(a1, c1)); // A1 → B1 → B2 → C1
        assert!(d.happened_before(a1, a2)); // along A's lane
        assert!(!d.happened_before(a2, b2) && !d.happened_before(b2, a2));
        assert!(!d.happened_before(b1, a1));
    }

    #[test]
    fn notes_follow_reading_order_and_light_the_path() {
        let (mut d, [a1, b1, b2, c1, a2]) = trace();
        d.callout_concurrent(a2, b2, "late"); // added first, but its later event comes last
        d.callout_before(a1, c1, "early");
        let notes = d.notes();
        assert_eq!(notes[0].callout.text.plain(), "early");
        assert_eq!(notes[0].events, BTreeSet::from([a1, b1, b2, c1]));
        assert_eq!(notes[0].messages, BTreeSet::from([0, 1]));
        assert_eq!(notes[1].n, 2);
        assert_eq!(notes[1].events, BTreeSet::from([a2, b2]));
        assert!(notes[1].messages.is_empty());
    }

    #[test]
    fn the_timeline_puts_each_note_right_after_its_later_event() {
        let (mut d, [a1, b1, b2, c1, a2]) = trace();
        d.callout_concurrent(a2, b2, "late");
        d.callout_before(a1, c1, "early");
        let t = d.timeline();
        assert_eq!(
            t.beats(),
            [
                Beat::Start,
                Beat::Event(a1),
                Beat::Event(b1),
                Beat::Event(b2),
                Beat::Event(c1),
                Beat::Note(1),
                Beat::Event(a2),
                Beat::Note(2),
                Beat::End
            ]
        );
        assert_eq!((t.event(c1), t.note(1), t.note(2), t.last()), (4, 5, 7, 8));
        assert_eq!(t.now(0), None);
        assert_eq!(t.now(5), Some(c1), "a note doesn't move the playhead");
        assert_eq!(t.now(t.last()), Some(a2));
    }

    #[test]
    fn a_lane_holds_its_latest_value_else_its_start() {
        let mut d = SpaceTime::new("t");
        let (a, b) = (d.lane("A"), d.lane("B"));
        d.start_value(a, "{}");
        let a1 = d.event(a, "A1", "{A:1}");
        d.event(b, "B1", "");
        let a2 = d.event(a, "A2", "");
        let t = d.timeline();
        let at = |lane, s| {
            d.lane_state(&t, lane, s)
                .map(|x| (x.event, x.value.plain(), x.before.map(Rich::plain)))
        };
        assert_eq!(at(a, 0), Some((None, "{}".into(), None)));
        assert_eq!(
            at(a, 1),
            Some((Some(a1), "{A:1}".into(), Some("{}".into())))
        );
        assert_eq!(
            at(a, t.event(a2)),
            at(a, 1),
            "an empty annotation holds nothing new"
        );
        assert_eq!(at(b, t.last()), None);
    }

    #[test]
    #[should_panic(expected = "already narrated")]
    fn an_event_is_narrated_once() {
        let mut d = SpaceTime::new("t");
        let a = d.lane("A");
        let a1 = d.event(a, "A1", "");
        d.narrate(a1, "ticks");
        d.narrate(a1, "ticks again");
    }

    #[test]
    #[should_panic(expected = "already has a start value")]
    fn a_start_value_is_set_once() {
        let mut d = SpaceTime::new("t");
        let a = d.lane("A");
        d.start_value(a, "{}");
        d.start_value(a, "{}");
    }

    fn clock(entries: &[(&str, u64)]) -> Rich {
        let mut line = vec![Span::value("{")];
        for (i, (a, n)) in entries.iter().enumerate() {
            if i > 0 {
                line.push(Span::value(", "));
            }
            line.push(Span::part(*a, "entry").also("actor").with("actor", *a));
            line.push(
                Span::part(format!(":{n}"), "entry")
                    .also("count")
                    .with("actor", *a),
            );
        }
        line.push(Span::value("}"));
        Rich { lines: vec![line] }
    }

    fn changed(r: &Rich) -> Vec<String> {
        r.lines
            .iter()
            .flatten()
            .filter(|s| s.roles.contains(&"changed"))
            .map(|s| s.text.clone())
            .collect()
    }

    #[test]
    fn mark_changes_marks_new_and_changed_parts_only() {
        let after = clock(&[("A", 2), ("B", 2)]).mark_changes(&clock(&[("B", 1)]));
        assert_eq!(changed(&after), ["A", ":2", ":2"]);
        let same = clock(&[("A", 1)]);
        assert!(changed(&same.mark_changes(&same)).is_empty());
    }

    #[test]
    fn mark_changes_matches_a_line_by_the_dot_it_starts_with() {
        // A causal context's line starts with its vector; a dot past the gap
        // comes later and keys nothing.
        let cloud = |a: &str, n: u64| {
            let dot = format!("{a}:{n}");
            [
                Span::part(a, "dot-id")
                    .also("actor")
                    .with("actor", a)
                    .with("dot", &dot),
                Span::part(format!(":{n}"), "dot-id")
                    .also("count")
                    .with("actor", a)
                    .with("dot", &dot),
            ]
        };
        let mut gap = clock(&[("A", 1)]);
        gap.lines[0].push(Span::value(" + dot "));
        gap.lines[0].extend(cloud("A", 3));
        assert_eq!(changed(&gap.mark_changes(&clock(&[("A", 1)]))), ["A", ":3"]);
    }

    #[test]
    fn mark_changes_ignores_a_field_names_padding() {
        let before = crate::label::fields(vec![("L", Rich::from(Span::part("1", "lamport")))]);
        let after = crate::label::fields(vec![
            ("phys", Rich::from(Span::part("10", "physical"))),
            ("L", Rich::from(Span::part("1", "lamport"))),
        ]);
        assert_eq!(changed(&after.mark_changes(&before)), ["phys\u{a0}", "10"]);
    }

    #[test]
    fn mark_changes_keeps_an_unchanged_sibling() {
        let entry = |a: &str, n: u64, v: &str| {
            let dot = format!("{a}:{n}");
            vec![
                Span::part(a, "dot-id")
                    .also("actor")
                    .with("actor", a)
                    .with("dot", &dot),
                Span::part(format!(":{n}"), "dot-id")
                    .also("count")
                    .with("actor", a)
                    .with("dot", &dot),
                Span::part("→", "sym"),
                Span::part(format!("{v:?}"), "str"),
            ]
        };
        let before = Rich {
            lines: vec![entry("B", 1, "b")],
        };
        let after = Rich {
            lines: vec![entry("A", 2, "a"), entry("B", 1, "b")],
        };
        assert_eq!(changed(&after.mark_changes(&before)), ["A", ":2", "\"a\""]);
    }

    #[test]
    #[should_panic(expected = "draws no path")]
    fn a_before_callout_needs_a_drawn_path() {
        let (mut d, [_, _, b2, _, a2]) = trace();
        d.callout_before(a2, b2, "");
    }

    #[test]
    fn rich_text_joins_and_flattens() {
        let r = rich!["a ", Span::part("A:1", "dot-id"), "\nb"];
        assert_eq!(r.lines.len(), 2);
        assert_eq!(r.plain(), "a A:1\nb");
        assert!(Rich::from("").is_empty());
    }
}
