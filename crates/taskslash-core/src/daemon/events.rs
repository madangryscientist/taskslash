//! The event stream: every subscriber gets each job event as a line of JSON, and a slow one never
//! slows a job. Each subscriber has a bounded queue that its own thread writes out; publishing only
//! tries to queue, and what does not fit is counted and reported as `dropped` before the next event
//! that does.

use super::Daemon;
use crate::protocol::{DaemonMessage, JobEvent, PROTOCOL_VERSION, send};
use crate::scheduler::lock;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

/// How many events a subscriber's queue holds before events for it are dropped.
pub(super) const QUEUE: usize = 1024;

/// One line, ready to write, shared by every subscriber it is queued for.
type Line = Arc<str>;

struct Subscriber {
    queue: SyncSender<Line>,
    /// Events not queued for it since the last one that was.
    dropped: u64,
}

/// The subscribers and the event counter.
#[derive(Default)]
pub(super) struct Bus {
    /// Held while an event is numbered and queued, so each subscriber gets them in `seq` order.
    subscribers: Mutex<(u64, Vec<Subscriber>)>,
}

impl Bus {
    /// A new subscriber's queue: it gets every event published from now on.
    pub(super) fn subscribe(&self) -> Receiver<Line> {
        let (queue, events) = sync_channel(QUEUE);
        lock(&self.subscribers)
            .1
            .push(Subscriber { queue, dropped: 0 });
        events
    }

    /// Numbers `event` and queues it for every subscriber, never waiting. A subscriber whose queue
    /// is full misses it (and is told how many it missed before the next one it gets); one whose
    /// writer is gone is removed.
    pub(super) fn publish(&self, event: JobEvent) {
        let mut guard = lock(&self.subscribers);
        let (seq, subscribers) = &mut *guard;
        *seq += 1;
        let seq = *seq;
        if subscribers.is_empty() {
            return;
        }
        let next = line(seq, event);
        subscribers.retain_mut(|subscriber| {
            if subscriber.dropped > 0 {
                let dropped = line(
                    seq - 1,
                    JobEvent::Dropped {
                        count: subscriber.dropped,
                    },
                );
                match subscriber.queue.try_send(dropped) {
                    Ok(()) => subscriber.dropped = 0,
                    Err(TrySendError::Full(_)) => {
                        subscriber.dropped += 1;
                        return true;
                    }
                    Err(TrySendError::Disconnected(_)) => return false,
                }
            }
            match subscriber.queue.try_send(Arc::clone(&next)) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    subscriber.dropped += 1;
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }
}

/// `event` as an `event` line numbered `seq`.
fn line(seq: u64, event: JobEvent) -> Line {
    let mut wire = Vec::new();
    let message = DaemonMessage::Event {
        protocol: PROTOCOL_VERSION,
        seq,
        event,
    };
    // Writing into a Vec fails only if serializing does, which these types never do.
    let _ = send(&mut wire, &message);
    String::from_utf8_lossy(&wire).into()
}

impl Daemon {
    /// A `subscribe` connection: `subscribed`, then its queue written out on this thread until a
    /// write fails (the subscriber hung up), which drops the queue so the next publish removes it.
    pub(super) fn serve_subscriber(&self, writer: &mut UnixStream) {
        let events = self.events.subscribe();
        eprintln!("subscriber connected");
        let subscribed = DaemonMessage::Subscribed {
            protocol: PROTOCOL_VERSION,
        };
        if send(writer, &subscribed).is_err() {
            return;
        }
        for line in events {
            if writer.write_all(line.as_bytes()).is_err() {
                break;
            }
        }
        eprintln!("subscriber gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::receive;
    use std::time::{Duration, Instant};

    fn finished(job: u64) -> JobEvent {
        JobEvent::Finished {
            job,
            key: "k".into(),
            exit_code: Some(0),
            wall_ms: 1,
            cached: None,
            moon_hash: None,
            run: Some(i64::try_from(job).unwrap()),
            log: None,
            at: "2026-11-17T00:00:00.000Z".into(),
        }
    }

    /// The `seq` and event of a queued line.
    fn read(line: &Line) -> (u64, JobEvent) {
        match receive(&mut line.as_bytes()).unwrap() {
            Some(DaemonMessage::Event {
                protocol: 1,
                seq,
                event,
            }) => (seq, event),
            other => panic!("not an event: {other:?}"),
        }
    }

    #[test]
    fn a_subscriber_that_never_reads_does_not_block_publish_and_is_told_what_it_missed() {
        let bus = Bus::default();
        let events = bus.subscribe();
        let began = Instant::now();
        for job in 1..=2000 {
            bus.publish(finished(job));
        }
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "publish waited: {:?}",
            began.elapsed()
        );
        let queued: Vec<Line> = events.try_iter().collect();
        assert_eq!(queued.len(), QUEUE);
        for (i, line) in queued.iter().enumerate() {
            let job = i as u64 + 1;
            assert_eq!(read(line), (job, finished(job)));
        }
        // The subscriber reads again: one `dropped` for the 976 it missed, then the next event.
        bus.publish(finished(2001));
        let resumed: Vec<Line> = events.try_iter().collect();
        assert_eq!(resumed.len(), 2);
        assert_eq!(read(&resumed[0]), (2000, JobEvent::Dropped { count: 976 }));
        assert_eq!(read(&resumed[1]), (2001, finished(2001)));
        // The count started again from 0.
        bus.publish(finished(2002));
        let next: Vec<Line> = events.try_iter().collect();
        assert_eq!(next.len(), 1);
        assert_eq!(read(&next[0]), (2002, finished(2002)));
    }

    #[test]
    fn a_queue_with_room_for_only_the_notice_drops_the_event_after_it() {
        let bus = Bus::default();
        let events = bus.subscribe();
        for job in 1..=QUEUE as u64 + 1 {
            bus.publish(finished(job));
        }
        // Room for one: the `dropped` notice goes in, the event after it does not.
        let _ = events.recv().unwrap();
        bus.publish(finished(QUEUE as u64 + 2));
        let rest: Vec<Line> = events.try_iter().collect();
        assert_eq!(rest.len(), QUEUE);
        assert_eq!(
            read(rest.last().unwrap()),
            (QUEUE as u64 + 1, JobEvent::Dropped { count: 1 })
        );
        bus.publish(finished(QUEUE as u64 + 3));
        let resumed: Vec<Line> = events.try_iter().collect();
        assert_eq!(
            read(&resumed[0]),
            (QUEUE as u64 + 2, JobEvent::Dropped { count: 1 })
        );
        assert_eq!(
            read(&resumed[1]),
            (QUEUE as u64 + 3, finished(QUEUE as u64 + 3))
        );
    }

    #[test]
    fn a_subscriber_whose_writer_is_gone_is_removed() {
        let bus = Bus::default();
        let gone = bus.subscribe();
        let kept = bus.subscribe();
        drop(gone);
        bus.publish(finished(1));
        assert_eq!(lock(&bus.subscribers).1.len(), 1);
        assert_eq!(read(&kept.recv().unwrap()), (1, finished(1)));
    }

    #[test]
    fn seq_counts_every_event_even_with_no_subscriber() {
        let bus = Bus::default();
        bus.publish(finished(1));
        let events = bus.subscribe();
        bus.publish(finished(2));
        assert_eq!(read(&events.recv().unwrap()), (2, finished(2)));
    }
}
