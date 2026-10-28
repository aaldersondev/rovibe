//! Who drives Roblox Studio. A playtest, a screenshot, the camera and the
//! game's input are one live session per place: two agents starting tests
//! or moving the camera at once spoil each other's results. The first agent
//! to act holds the place; the others are told who has it, and can wait in
//! line with `studio_wait` instead of retrying.

use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::{Duration, Instant},
};

use tokio::sync::Notify;

/// A hold lapses this long after its holder's last Studio action…
const IDLE: Duration = Duration::from_secs(30);
/// …but never sooner than this after it was taken, so a short pause between
/// two steps doesn't hand the place over.
const MINIMUM: Duration = Duration::from_secs(60);
/// While the holder's playtest runs, the place stays its own for longer:
/// reading the console or thinking about a screenshot is part of the test.
const PLAYING: Duration = Duration::from_secs(180);
/// A waiting agent that stopped calling `studio_wait` loses its place in line.
const QUEUE_STALE: Duration = Duration::from_secs(45);

#[derive(Clone)]
struct Hold {
    session: String,
    title: String,
    since: Instant,
    last: Instant,
    playing: bool,
}

impl Hold {
    fn expired(&self, now: Instant) -> bool {
        let idle = if self.playing { PLAYING } else { IDLE };
        now >= self.last + idle && now >= self.since + MINIMUM
    }
}

struct Waiter {
    session: String,
    seen: Instant,
}

#[derive(Default)]
struct Place {
    hold: Option<Hold>,
    queue: VecDeque<Waiter>,
}

/// Who holds the place, in words for the agent that was refused.
#[derive(Debug)]
pub struct Busy {
    pub title: String,
    pub held_for: Duration,
    pub playing: bool,
    pub waiting: usize,
}

impl Busy {
    pub fn message(&self) -> String {
        format!(
            "Studio est piloté par {} depuis {} s{}{}. Un seul agent à la fois lance des tests, prend des captures ou joue au jeu dans une place. Appelle `studio_wait` : il te rend la main dès que c'est ton tour, avec Studio déjà réservé pour toi. En attendant, tu peux lire la place (`get_tree`, `search`, `get_instance`, `run_luau` en contexte edit) ou travailler sur les fichiers.",
            self.title,
            self.held_for.as_secs(),
            if self.playing { ", avec un test en cours" } else { "" },
            match self.waiting {
                0 => String::new(),
                n => format!(", {n} agent(s) déjà en attente"),
            }
        )
    }
}

pub enum Waited {
    Granted,
    /// Still not its turn when the wait ran out: its position in line,
    /// counting from 1.
    Queued(usize, Busy),
}

#[derive(Default)]
pub struct Drives {
    places: Mutex<HashMap<String, Place>>,
    changed: Notify,
}

impl Drives {
    /// Takes or renews the hold for `session`. `gone` says whether a session
    /// no longer exists, so a closed agent never keeps a place.
    pub fn acquire(&self, place: &str, session: &str, title: &str, gone: impl Fn(&str) -> bool) -> Result<(), Busy> {
        self.acquire_at(place, session, title, gone, Instant::now())
    }

    fn acquire_at(
        &self,
        place: &str,
        session: &str,
        title: &str,
        gone: impl Fn(&str) -> bool,
        now: Instant,
    ) -> Result<(), Busy> {
        let mut places = self.places.lock().unwrap();
        let entry = places.entry(place.to_owned()).or_default();
        let freed = tidy(entry, &gone, now);

        if let Some(hold) = entry.hold.as_mut().filter(|hold| hold.session == session) {
            hold.last = now;
            return Ok(());
        }
        if let Some(hold) = &entry.hold {
            return Err(busy(hold, entry.queue.len(), now));
        }
        // A free place goes to whoever is first in line; someone who never
        // queued only gets it when nobody is waiting.
        let first = entry.queue.front().map(|waiter| waiter.session.clone());
        if first.as_deref().is_some_and(|first| first != session) {
            let waiting = entry.queue.len();
            drop(places);
            if freed {
                self.changed.notify_waiters();
            }
            return Err(Busy {
                title: "un autre agent, qui attendait avant toi".to_owned(),
                held_for: Duration::ZERO,
                playing: false,
                waiting,
            });
        }
        if first.is_some() {
            entry.queue.pop_front();
        }
        entry.hold = Some(Hold {
            session: session.to_owned(),
            title: title.to_owned(),
            since: now,
            last: now,
            playing: false,
        });
        Ok(())
    }

    /// Waits in line until `session` holds the place, or `limit` runs out.
    pub async fn wait(&self, place: &str, session: &str, title: &str, gone: impl Fn(&str) -> bool, limit: Duration) -> Waited {
        let deadline = Instant::now() + limit;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let mut places = self.places.lock().unwrap();
                let entry = places.entry(place.to_owned()).or_default();
                let now = Instant::now();
                match entry.queue.iter_mut().find(|waiter| waiter.session == session) {
                    Some(waiter) => waiter.seen = now,
                    None if entry.hold.as_ref().is_some_and(|hold| hold.session == session) => {}
                    None => entry.queue.push_back(Waiter { session: session.to_owned(), seen: now }),
                }
            }

            match self.acquire(place, session, title, &gone) {
                Ok(()) => {
                    return Waited::Granted;
                }
                Err(busy) => {
                    if Instant::now() >= deadline {
                        let position = self.position(place, session).unwrap_or(1);
                        return Waited::Queued(position, busy);
                    }
                }
            }
            // Expiries happen with nobody calling: look again every second.
            let _ = tokio::time::timeout(Duration::from_secs(1).min(deadline - Instant::now()), notified).await;
        }
    }

    fn position(&self, place: &str, session: &str) -> Option<usize> {
        let places = self.places.lock().unwrap();
        places
            .get(place)?
            .queue
            .iter()
            .position(|waiter| waiter.session == session)
            .map(|index| index + 1)
    }

    /// Marks the holder's playtest as running or over.
    pub fn set_playing(&self, place: &str, session: &str, playing: bool) {
        let mut places = self.places.lock().unwrap();
        if let Some(hold) = places
            .get_mut(place)
            .and_then(|entry| entry.hold.as_mut())
            .filter(|hold| hold.session == session)
        {
            hold.playing = playing;
            hold.last = Instant::now();
        }
    }

    /// Gives the place up. Returns whether `session` held it.
    pub fn release(&self, place: &str, session: &str) -> bool {
        let mut places = self.places.lock().unwrap();
        let Some(entry) = places.get_mut(place) else {
            return false;
        };
        entry.queue.retain(|waiter| waiter.session != session);
        let held = entry.hold.as_ref().is_some_and(|hold| hold.session == session);
        if held {
            entry.hold = None;
        }
        drop(places);
        self.changed.notify_waiters();
        held
    }

    /// Who holds the place now, for `studio_status` and the app.
    pub fn holder(&self, place: &str, gone: impl Fn(&str) -> bool) -> Option<(String, String, bool)> {
        let mut places = self.places.lock().unwrap();
        let entry = places.get_mut(place)?;
        tidy(entry, &gone, Instant::now());
        entry
            .hold
            .as_ref()
            .map(|hold| (hold.session.clone(), hold.title.clone(), hold.playing))
    }
}

/// Drops lapsed holds and stale waiters. Returns whether the place was freed.
fn tidy(entry: &mut Place, gone: &impl Fn(&str) -> bool, now: Instant) -> bool {
    entry
        .queue
        .retain(|waiter| now < waiter.seen + QUEUE_STALE && !gone(&waiter.session));
    let lapsed = entry
        .hold
        .as_ref()
        .is_some_and(|hold| hold.expired(now) || gone(&hold.session));
    if lapsed {
        entry.hold = None;
    }
    lapsed
}

fn busy(hold: &Hold, waiting: usize, now: Instant) -> Busy {
    Busy {
        title: hold.title.clone(),
        held_for: now - hold.since,
        playing: hold.playing,
        waiting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive(_: &str) -> bool {
        false
    }

    #[test]
    fn the_first_agent_holds_the_place_and_the_second_is_told_who() {
        let drives = Drives::default();
        let now = Instant::now();
        assert!(drives.acquire_at("p", "a", "Agent A", alive, now).is_ok());
        let refused = drives.acquire_at("p", "b", "Agent B", alive, now).unwrap_err();
        assert!(refused.message().contains("Agent A"), "{}", refused.message());
        // The holder itself keeps going.
        assert!(drives.acquire_at("p", "a", "Agent A", alive, now).is_ok());
        // Another place is another driver.
        assert!(drives.acquire_at("q", "b", "Agent B", alive, now).is_ok());
    }

    #[test]
    fn a_hold_lapses_after_a_pause_but_not_right_after_it_was_taken() {
        let drives = Drives::default();
        let start = Instant::now();
        drives.acquire_at("p", "a", "A", alive, start).unwrap();
        // Idle for 40 s but taken 40 s ago: still the minimum.
        assert!(drives.acquire_at("p", "b", "B", alive, start + Duration::from_secs(40)).is_err());
        // 61 s after taking it and more than 30 s idle: free.
        assert!(drives.acquire_at("p", "b", "B", alive, start + Duration::from_secs(61)).is_ok());
    }

    #[test]
    fn a_running_test_keeps_the_place_longer() {
        let drives = Drives::default();
        let start = Instant::now();
        drives.acquire_at("p", "a", "A", alive, start).unwrap();
        drives.set_playing("p", "a", true);
        assert!(drives.acquire_at("p", "b", "B", alive, start + Duration::from_secs(120)).is_err());
        assert!(drives.acquire_at("p", "b", "B", alive, start + Duration::from_secs(200)).is_ok());
    }

    #[test]
    fn a_closed_agent_never_keeps_the_place() {
        let drives = Drives::default();
        drives.acquire("p", "a", "A", alive).unwrap();
        assert!(drives.acquire("p", "b", "B", |session| session == "a").is_ok());
    }

    #[tokio::test]
    async fn waiting_agents_are_served_in_line_order() {
        let drives = std::sync::Arc::new(Drives::default());
        drives.acquire("p", "a", "A", alive).unwrap();

        let first = drives.clone();
        let b = tokio::spawn(async move { matches!(first.wait("p", "b", "B", alive, Duration::from_secs(5)).await, Waited::Granted) });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let second = drives.clone();
        let c = tokio::spawn(async move { second.wait("p", "c", "C", alive, Duration::from_millis(600)).await });
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Someone who never queued can't jump the line once the place frees.
        assert!(drives.release("p", "a"));
        assert!(b.await.unwrap(), "le premier arrivé obtient la place");
        match c.await.unwrap() {
            Waited::Queued(position, busy) => {
                assert_eq!(position, 1);
                assert_eq!(busy.title, "B");
            }
            Waited::Granted => panic!("le second ne doit pas passer tant que le premier tient la place"),
        }
        assert!(drives.acquire("p", "d", "D", alive).is_err());
    }
}
