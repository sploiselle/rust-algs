//! # An airline reservation system
//!
//! The running example of Lamport's *The Implementation of Reliable
//! Distributed Multiprocess Systems* (1978), written as a [`UserMachine`].
//!
//! The paper opens with why this is harder than it looks. Run the system on
//! three computers that each vote on every request, and grant a request that
//! gets two yes votes. Three requests for a flight's last two seats arrive at
//! the three computers in different orders, and each computer votes yes on
//! the first two it sees. Every request can collect two votes, and three
//! passengers get two seats (`voting_on_each_request_overfills_a_flight`).
//! Run as a user machine, every copy executes the three requests in the same
//! order, and every copy books the same two passengers and waitlists the third
//! (`bookkeepers_never_overfill_a_flight`).
//!
//! The machine's state is a set of flights, each with a capacity, the
//! passengers holding seats, and a waiting list. The paper's flights also
//! have dates; a flight number here stands for a flight on a date. The
//! machine's commands are the first two rows of the paper's Table 1:
//!
//! - *Request 1 seat for Jones on Flight 221*: "Flight filled, Jones placed on
//!   waiting list."
//! - *Cancel reservation for Smith on Flight 221*: "Smith's reservation
//!   cancelled; Jones moved from waiting list onto Flight 221."
//!
//! The table's third row, *How many seats are left on Flight 123?*, is a
//! query: it changes nothing, so it needn't be voted on. One bookkeeper
//! answers it from its own copy, as of the latest time that copy reflects
//! ([`Airline::seats_left`], [`Bookkeeper::known`](super::Bookkeeper::known)).
//! That's the second of the three ways the paper's Section 4.2 gives.

use super::UserMachine;
use std::collections::{BTreeMap, VecDeque};

/// A request from a reservation station.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Command {
    /// Request a seat for `passenger` on `flight`.
    Reserve { flight: u32, passenger: String },
    /// Give up `passenger`'s seat on `flight`, or its place on the waiting
    /// list.
    Cancel { flight: u32, passenger: String },
}

impl Command {
    pub fn reserve(flight: u32, passenger: &str) -> Self {
        Self::Reserve {
            flight,
            passenger: passenger.to_owned(),
        }
    }

    pub fn cancel(flight: u32, passenger: &str) -> Self {
        Self::Cancel {
            flight,
            passenger: passenger.to_owned(),
        }
    }
}

/// What executing a [`Command`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// The passenger has a seat.
    Booked,
    /// The flight is full, so the passenger is on its waiting list.
    Waitlisted,
    /// The passenger's seat or place is given up, and if a seat came free,
    /// `promoted`, first on the waiting list, now has it.
    Cancelled { promoted: Option<String> },
    /// No such flight. Nothing changed.
    NoSuchFlight,
    /// A reservation for a passenger who already has a seat or a place on the
    /// waiting list. Nothing changed.
    AlreadyListed,
    /// A cancellation for a passenger with neither. Nothing changed.
    NotListed,
}

/// Every flight, with who holds its seats and who's waiting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Airline {
    flights: BTreeMap<u32, Flight>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Flight {
    capacity: usize,
    /// In the order they were booked.
    seated: Vec<String>,
    /// First come, first served.
    waiting: VecDeque<String>,
}

impl Airline {
    /// An airline flying each `(flight, capacity)`, with nobody booked.
    pub fn new(flights: impl IntoIterator<Item = (u32, usize)>) -> Self {
        let flights = flights
            .into_iter()
            .map(|(number, capacity)| {
                let flight = Flight {
                    capacity,
                    seated: Vec::new(),
                    waiting: VecDeque::new(),
                };
                (number, flight)
            })
            .collect();
        Self { flights }
    }

    /// The seats left on `flight`, or `None` if there's no such flight.
    pub fn seats_left(&self, flight: u32) -> Option<usize> {
        let f = self.flights.get(&flight)?;
        Some(f.capacity - f.seated.len())
    }

    /// Who holds seats on `flight`, in the order they were booked.
    pub fn seated(&self, flight: u32) -> Option<impl Iterator<Item = &str>> {
        Some(self.flights.get(&flight)?.seated.iter().map(String::as_str))
    }

    /// Who's waiting for a seat on `flight`, first in line first.
    pub fn waiting(&self, flight: u32) -> Option<impl Iterator<Item = &str>> {
        Some(
            self.flights
                .get(&flight)?
                .waiting
                .iter()
                .map(String::as_str),
        )
    }
}

impl UserMachine for Airline {
    type Command = Command;
    type Response = Response;

    fn execute(&mut self, command: &Command) -> Response {
        match command {
            Command::Reserve { flight, passenger } => {
                let Some(f) = self.flights.get_mut(flight) else {
                    return Response::NoSuchFlight;
                };
                if f.seated.contains(passenger) || f.waiting.contains(passenger) {
                    Response::AlreadyListed
                } else if f.seated.len() < f.capacity {
                    f.seated.push(passenger.clone());
                    Response::Booked
                } else {
                    f.waiting.push_back(passenger.clone());
                    Response::Waitlisted
                }
            }
            Command::Cancel { flight, passenger } => {
                let Some(f) = self.flights.get_mut(flight) else {
                    return Response::NoSuchFlight;
                };
                if let Some(i) = f.seated.iter().position(|p| p == passenger) {
                    f.seated.remove(i);
                    let promoted = f.waiting.pop_front();
                    f.seated.extend(promoted.clone());
                    Response::Cancelled { promoted }
                } else if let Some(i) = f.waiting.iter().position(|p| p == passenger) {
                    f.waiting.remove(i);
                    Response::Cancelled { promoted: None }
                } else {
                    Response::NotListed
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::sim::Sim;
    use crate::state_machine::{Config, Stamped};

    fn seated(airline: &Airline, flight: u32) -> Vec<&str> {
        airline.seated(flight).unwrap().collect()
    }

    fn waiting(airline: &Airline, flight: u32) -> Vec<&str> {
        airline.waiting(flight).unwrap().collect()
    }

    /// Table 1, on one copy: a full flight waitlists Jones, then Brown, and
    /// Smith's cancellation gives the seat to Jones, first in line.
    #[test]
    fn table_1() {
        let mut airline = Airline::new([(221, 1), (123, 29)]);
        assert_eq!(
            airline.execute(&Command::reserve(221, "Smith")),
            Response::Booked
        );
        for p in ["Jones", "Brown"] {
            assert_eq!(
                airline.execute(&Command::reserve(221, p)),
                Response::Waitlisted
            );
        }
        assert_eq!(
            airline.execute(&Command::cancel(221, "Smith")),
            Response::Cancelled {
                promoted: Some("Jones".into())
            }
        );
        assert_eq!(seated(&airline, 221), ["Jones"]);
        assert_eq!(waiting(&airline, 221), ["Brown"]);

        for p in ["Brown", "Green"] {
            assert_eq!(airline.execute(&Command::reserve(123, p)), Response::Booked);
        }
        assert_eq!(airline.seats_left(123), Some(27));
    }

    /// An invalid command gets an error response and changes nothing. Leaving
    /// the waiting list is valid, but frees no seat.
    #[test]
    fn invalid_commands_change_nothing() {
        let mut airline = Airline::new([(221, 1)]);
        for p in ["Smith", "Jones", "Brown"] {
            airline.execute(&Command::reserve(221, p));
        }
        let before = airline.clone();
        for (command, response) in [
            (Command::reserve(999, "Smith"), Response::NoSuchFlight),
            (Command::cancel(999, "Smith"), Response::NoSuchFlight),
            (Command::reserve(221, "Smith"), Response::AlreadyListed),
            (Command::reserve(221, "Jones"), Response::AlreadyListed),
            (Command::cancel(221, "Green"), Response::NotListed),
        ] {
            assert_eq!(airline.execute(&command), response, "{command:?}");
        }
        assert_eq!(airline, before);

        assert_eq!(
            airline.execute(&Command::cancel(221, "Jones")),
            Response::Cancelled { promoted: None }
        );
        assert_eq!(seated(&airline, 221), ["Smith"]);
        assert_eq!(waiting(&airline, 221), ["Brown"]);
    }

    /// A cancellation leaves everyone else in the order they booked.
    #[test]
    fn seats_stay_in_booking_order() {
        let mut airline = Airline::new([(221, 4)]);
        for p in ["Smith", "Jones", "Brown", "Green"] {
            airline.execute(&Command::reserve(221, p));
        }
        airline.execute(&Command::cancel(221, "Jones"));
        assert_eq!(seated(&airline, 221), ["Smith", "Brown", "Green"]);
    }

    /// The paper's Section 1.1. Each of three computers votes yes on the first
    /// two requests it hears of for the last two seats, and a request with two
    /// yes votes is granted. The requests reach the computers in different
    /// orders, each collects two votes, and three passengers hold two seats.
    #[test]
    fn voting_on_each_request_overfills_a_flight() {
        let requests = ["Jones", "Smith", "Brown"];
        let heard_first = [["Jones", "Smith"], ["Smith", "Brown"], ["Brown", "Jones"]];
        let granted: Vec<&str> = requests
            .into_iter()
            .filter(|r| heard_first.iter().filter(|yes| yes.contains(r)).count() >= 2)
            .collect();
        assert_eq!(granted, requests);
    }

    /// The same three requests, issued at once at three different
    /// bookkeepers, which hear them in different orders. Every copy executes
    /// them in stamp order, so every copy books the same two passengers and
    /// waitlists the third. It holds with a bookkeeper crashed, too.
    #[test]
    fn bookkeepers_never_overfill_a_flight() {
        for crashed in [None, Some('C')] {
            let config = Config::majority(['A', 'B', 'C'], 10, 40);
            let mut sim = Sim::new(config, Airline::new([(221, 2)]), 7);
            if let Some(p) = crashed {
                sim.crash(p);
            }
            let requests = [("Jones", 'A'), ("Smith", 'B'), ("Brown", 'C')];
            for (passenger, to) in requests {
                let command = Stamped::new(5, Command::reserve(221, passenger));
                sim.submit(6, to, command.clone());
                // Users may send a command to more than one bookkeeper.
                sim.submit(8, if to == 'A' { 'B' } else { 'A' }, command);
            }
            sim.run_until(100);
            sim.assert_agree("overfill");

            for p in sim.ids().into_iter().filter(|&p| Some(p) != crashed) {
                let book = sim.book(p);
                // Commands issued at the same time run in command order.
                let responses: Vec<_> = book
                    .executed()
                    .iter()
                    .map(|(c, r)| (c.command().clone(), r.clone()))
                    .collect();
                assert_eq!(
                    responses,
                    [
                        (Command::reserve(221, "Brown"), Response::Booked),
                        (Command::reserve(221, "Jones"), Response::Booked),
                        (Command::reserve(221, "Smith"), Response::Waitlisted),
                    ],
                    "{p}"
                );
                assert_eq!(book.machine().seats_left(221), Some(0));
            }
        }
    }

    /// A query is answered from one copy, as of the time it reflects: a
    /// bookkeeper that hasn't decided a reservation yet still answers, with
    /// the seats left before it.
    #[test]
    fn a_query_reads_one_copy_as_of_its_known_time() {
        let config = Config::majority(['A', 'B', 'C'], 10, 40);
        let mut sim = Sim::new(config, Airline::new([(123, 29)]), 3);
        sim.submit(11, 'A', Stamped::new(10, Command::reserve(123, "Green")));
        sim.run_until(15);
        let a = sim.book('A');
        assert!(a.known() < 10);
        assert_eq!(a.machine().seats_left(123), Some(29));

        sim.run_until(60);
        let a = sim.book('A');
        assert!(a.known() >= 10);
        assert_eq!(a.machine().seats_left(123), Some(28));
    }
}
