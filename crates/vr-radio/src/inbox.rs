//! Events sent from native callbacks (window procedure, tray, hotkeys) to
//! the window, without polling: the window waits on the receiver and wakes
//! only when something was sent.

use std::sync::Mutex;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// A channel whose receiver is taken once, by the subscription that reads it.
pub struct Inbox<T> {
    tx: UnboundedSender<T>,
    rx: Mutex<Option<UnboundedReceiver<T>>>,
}

impl<T> Inbox<T> {
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    /// Never blocks; dropped when nobody reads any more.
    pub fn send(&self, event: T) {
        let _ = self.tx.send(event);
    }

    /// The receiver, for the one reader. `None` once taken.
    pub fn take(&self) -> Option<UnboundedReceiver<T>> {
        self.rx.lock().ok()?.take()
    }

    /// Events sent so far, while nobody has taken the receiver (tests).
    pub fn drain(&self) -> Vec<T> {
        let mut out = Vec::new();
        if let Ok(mut rx) = self.rx.lock()
            && let Some(rx) = rx.as_mut()
        {
            while let Ok(e) = rx.try_recv() {
                out.push(e);
            }
        }
        out
    }
}

impl<T> Default for Inbox<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Forwards everything from the inbox's receiver as messages until the
/// window is gone. Waits forever when the receiver was already taken.
pub async fn forward<T, M>(
    rx: Option<UnboundedReceiver<T>>,
    mut out: iced::futures::channel::mpsc::Sender<M>,
    message: impl Fn(T) -> M,
) {
    use iced::futures::SinkExt;
    let Some(mut rx) = rx else {
        return std::future::pending().await;
    };
    while let Some(e) = rx.recv().await {
        if out.send(message(e)).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_receiver_is_taken_once_and_drain_reads_what_was_sent() {
        let inbox = Inbox::new();
        inbox.send(1);
        inbox.send(2);
        assert_eq!(inbox.drain(), vec![1, 2]);
        assert_eq!(inbox.drain(), Vec::<i32>::new());
        inbox.send(3);
        let mut rx = inbox.take().unwrap();
        assert!(inbox.take().is_none());
        assert_eq!(rx.try_recv().unwrap(), 3);
        // Taken: nothing left to drain.
        inbox.send(4);
        assert_eq!(inbox.drain(), Vec::<i32>::new());
        assert_eq!(rx.try_recv().unwrap(), 4);
    }

    #[tokio::test]
    async fn forward_sends_each_event_as_a_message() {
        use iced::futures::StreamExt;
        let inbox = Inbox::new();
        let rx = inbox.take();
        let (out, mut messages) = iced::futures::channel::mpsc::channel(4);
        inbox.send(7);
        drop(inbox);
        forward(rx, out, |n: i32| n * 10).await;
        assert_eq!(messages.next().await, Some(70));
        assert_eq!(messages.next().await, None);
    }
}
