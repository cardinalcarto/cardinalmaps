//! A queue of jobs that its threads take in the order the caller picks at the time they
//! take one, rather than the order the jobs came in: the tile workers and fetchers take
//! the tiles the view needs most first, as that changes from frame to frame.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

pub struct Queue<J> {
    state: Mutex<State<J>>,
    changed: Condvar,
}

struct State<J> {
    jobs: VecDeque<J>,
    /// No more jobs are taken: the threads stop.
    closed: bool,
    /// Some thread is waiting with jobs queued that it didn't pick.
    passed_over: bool,
}

impl<J> Default for Queue<J> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                jobs: VecDeque::new(),
                closed: false,
                passed_over: false,
            }),
            changed: Condvar::new(),
        }
    }
}

impl<J> Queue<J> {
    /// Add a job. Dropped once the queue is closed.
    pub fn push(&self, job: J) {
        let mut state = self.state.lock().unwrap();
        if !state.closed {
            state.jobs.push_back(job);
            self.changed.notify_one();
        }
    }

    /// Have threads that passed over the queued jobs look again, as when what `pick`
    /// says may have changed.
    pub fn wake(&self) {
        // Under the lock, so that a thread between picking nothing and waiting doesn't miss it.
        let mut state = self.state.lock().unwrap();
        if std::mem::take(&mut state.passed_over) {
            self.changed.notify_all();
        }
    }

    /// Stop the threads: [`Self::pop`] returns `None` from now on.
    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.changed.notify_all();
    }

    /// Take the job `pick` chooses from those queued, oldest first, waiting while it
    /// chooses none (or none are queued). `None` once the queue is closed.
    pub fn pop(&self, mut pick: impl FnMut(&VecDeque<J>) -> Option<usize>) -> Option<J> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.closed {
                return None;
            }
            if !state.jobs.is_empty() {
                match pick(&state.jobs) {
                    Some(i) => return state.jobs.remove(i),
                    None => state.passed_over = true,
                }
            }
            state = self.changed.wait(state).unwrap();
        }
    }
}

/// The index of the job with the lowest key, the oldest of equals, skipping jobs whose
/// key is `None`.
pub fn lowest<J, K: Ord>(
    jobs: &VecDeque<J>,
    mut key: impl FnMut(&J) -> Option<K>,
) -> Option<usize> {
    let mut best: Option<(usize, K)> = None;
    for (i, job) in jobs.iter().enumerate() {
        if let Some(k) = key(job)
            && best.as_ref().is_none_or(|(_, b)| k < *b)
        {
            best = Some((i, k));
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn takes_the_lowest_key_oldest_first() {
        let q = Queue::default();
        for job in [(3, 'a'), (1, 'b'), (2, 'c'), (1, 'd')] {
            q.push(job);
        }
        let order: Vec<char> = (0..4)
            .map(|_| q.pop(|jobs| lowest(jobs, |j| Some(j.0))).unwrap().1)
            .collect();
        assert_eq!(order, ['b', 'd', 'c', 'a']);
    }

    #[test]
    fn waits_while_nothing_is_picked_until_woken() {
        let q = Arc::new(Queue::default());
        let allowed = Arc::new(Mutex::new(false));
        q.push(7);
        let (q2, allowed2) = (q.clone(), allowed.clone());
        let taker = thread::spawn(move || {
            q2.pop(|jobs| lowest(jobs, |_| allowed2.lock().unwrap().then_some(0)))
        });
        thread::sleep(Duration::from_millis(50));
        assert!(!taker.is_finished());
        *allowed.lock().unwrap() = true;
        q.wake();
        assert_eq!(taker.join().unwrap(), Some(7));
    }

    #[test]
    fn closing_stops_waiting_threads() {
        let q = Arc::new(Queue::<u8>::default());
        let q2 = q.clone();
        let taker = thread::spawn(move || q2.pop(|jobs| lowest(jobs, |_| Some(0))));
        thread::sleep(Duration::from_millis(20));
        q.close();
        assert_eq!(taker.join().unwrap(), None);
        q.push(1);
        assert_eq!(q.pop(|_| Some(0)), None);
    }
}
