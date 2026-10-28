use std::collections::VecDeque;
use std::sync::{Mutex, RwLock};

use futures::channel::oneshot;

/// How many messages the queue keeps for clients that resume a dropped
/// stream. Upstream kept every message for the whole session, so a long
/// session's memory only grew; a few thousand cover any realistic gap.
pub const RETAINED_MESSAGES: usize = 4096;

struct History<T> {
    /// Cursor of the first retained message.
    first: u32,
    messages: VecDeque<T>,
}

impl<T> History<T> {
    fn cursor(&self) -> u32 {
        self.first + self.messages.len() as u32
    }
}

/// A message queue with a bounded history that can be subscribed to.
///
/// Definitely non-optimal. This would ideally be a lockless mpmc queue.
pub struct MessageQueue<T> {
    history: RwLock<History<T>>,
    message_listeners: Mutex<Vec<Listener<T>>>,
    capacity: usize,
}

impl<T: Clone> Default for MessageQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> MessageQueue<T> {
    pub fn new() -> MessageQueue<T> {
        Self::with_capacity(RETAINED_MESSAGES)
    }

    pub fn with_capacity(capacity: usize) -> MessageQueue<T> {
        MessageQueue {
            history: RwLock::new(History {
                first: 0,
                messages: VecDeque::new(),
            }),
            message_listeners: Mutex::new(Vec::new()),
            capacity: capacity.max(1),
        }
    }

    pub fn push_messages(&self, new_messages: &[T]) {
        let mut message_listeners = self.message_listeners.lock().unwrap();
        let mut history = self.history.write().unwrap();
        history.messages.extend(new_messages.iter().cloned());

        let mut remaining_listeners = Vec::new();

        for listener in message_listeners.drain(..) {
            match fire_listener_if_ready(&history, listener) {
                Ok(_) => {}
                Err(listener) => remaining_listeners.push(listener),
            }
        }

        // Trimmed only after the listeners above had their messages.
        while history.messages.len() > self.capacity {
            history.messages.pop_front();
            history.first += 1;
        }

        // Without this annotation, Rust gets confused since the first argument
        // is a MutexGuard, but the second is a Vec.
        *message_listeners = remaining_listeners;
    }

    /// Subscribe to any messages occurring after the given message cursor.
    /// A cursor older than the history gets what is still retained: callers
    /// check `oldest_cursor` first when missing messages matters.
    pub fn subscribe(&self, cursor: u32) -> oneshot::Receiver<(u32, Vec<T>)> {
        let (sender, receiver) = oneshot::channel();

        let listener = {
            let listener = Listener { sender, cursor };

            let history = self.history.read().unwrap();

            match fire_listener_if_ready(&history, listener) {
                Ok(_) => return receiver,
                Err(listener) => listener,
            }
        };

        let mut message_listeners = self.message_listeners.lock().unwrap();
        message_listeners.push(listener);

        receiver
    }

    /// Subscribe to any messages being pushed into the queue.
    ///
    /// This method is only useful in tests. Non-test code should use subscribe
    /// instead.
    #[cfg(test)]
    #[allow(unused)]
    pub fn subscribe_any(&self) -> oneshot::Receiver<(u32, Vec<T>)> {
        self.subscribe(self.cursor())
    }

    pub fn cursor(&self) -> u32 {
        self.history.read().unwrap().cursor()
    }

    /// The oldest cursor a client can still resume from without missing a
    /// message.
    pub fn oldest_cursor(&self) -> u32 {
        self.history.read().unwrap().first
    }
}

struct Listener<T> {
    sender: oneshot::Sender<(u32, Vec<T>)>,
    cursor: u32,
}

fn fire_listener_if_ready<T: Clone>(
    history: &History<T>,
    listener: Listener<T>,
) -> Result<(), Listener<T>> {
    let current_cursor = history.cursor();

    if listener.cursor < current_cursor {
        let skip = listener.cursor.saturating_sub(history.first) as usize;
        let new_messages = history.messages.iter().skip(skip).cloned().collect();
        let _ = listener.sender.send((current_cursor, new_messages));
        Ok(())
    } else {
        Err(listener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subscriber_gets_what_came_after_its_cursor() {
        let queue = MessageQueue::with_capacity(10);
        queue.push_messages(&[1, 2, 3]);
        let mut receiver = queue.subscribe(1);
        assert_eq!(receiver.try_recv().unwrap(), Some((3, vec![2, 3])));
        assert_eq!(queue.oldest_cursor(), 0);
    }

    #[test]
    fn the_history_is_bounded_and_says_where_it_starts() {
        let queue = MessageQueue::with_capacity(3);
        queue.push_messages(&[1, 2, 3, 4, 5]);
        assert_eq!(queue.cursor(), 5);
        assert_eq!(queue.oldest_cursor(), 2);
        let mut receiver = queue.subscribe(3);
        assert_eq!(receiver.try_recv().unwrap(), Some((5, vec![4, 5])));
    }

    #[test]
    fn a_waiting_subscriber_gets_a_batch_larger_than_the_history() {
        let queue = MessageQueue::with_capacity(2);
        let mut receiver = queue.subscribe(0);
        queue.push_messages(&[1, 2, 3]);
        assert_eq!(receiver.try_recv().unwrap(), Some((3, vec![1, 2, 3])));
        assert_eq!(queue.oldest_cursor(), 1);
    }
}
