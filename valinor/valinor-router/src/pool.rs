//! A bounded pool of Thor's path search workspaces.
//!
//! A [`Workspace`] holds a search's labels, queues, and edge status, and keeps their storage
//! between requests so that searches don't allocate as they go.
//! Requests take one from the pool and give it back when they are done.
//! At most `limit` workspaces are ever made (lazily, as concurrent requests need them);
//! when all are in use, a request waits for one to come back.
//! So the pool also bounds how many path searches run at once, and their memory.
//! A workspace is trimmed to its [`Retention`] when it comes back,
//! so an idle pool holds at most that much search storage per workspace.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use valinor_thor::{Retention, Workspace};

struct State {
    idle: Vec<Workspace>,
    created: usize,
}

pub(crate) struct WorkspacePool {
    state: Mutex<State>,
    returned: Condvar,
    limit: usize,
    retention: Retention,
}

impl WorkspacePool {
    pub(crate) fn new(limit: usize, retention: Retention) -> Self {
        Self {
            state: Mutex::new(State {
                idle: Vec::with_capacity(limit),
                created: 0,
            }),
            returned: Condvar::new(),
            limit,
            retention,
        }
    }

    /// The lock never guards a half-made change (nothing in it can panic midway),
    /// so a poisoned lock is still consistent.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes an idle workspace, makes one if fewer than the limit exist,
    /// or waits for one to be returned.
    pub(crate) fn take(&self) -> PooledWorkspace<'_> {
        let mut state = self.lock();
        loop {
            if let Some(workspace) = state.idle.pop() {
                return PooledWorkspace::new(self, workspace);
            }
            if state.created < self.limit {
                state.created += 1;
                // Don't hold the lock while the workspace allocates.
                drop(state);
                return PooledWorkspace::new(self, Workspace::new(self.retention));
            }
            state = self
                .returned
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    #[cfg(test)]
    pub(crate) fn created(&self) -> usize {
        self.lock().created
    }

    #[cfg(test)]
    pub(crate) fn idle(&self) -> usize {
        self.lock().idle.len()
    }
}

/// A workspace taken from the pool, given back when dropped (even if the request panics).
pub(crate) struct PooledWorkspace<'a> {
    pool: &'a WorkspacePool,
    workspace: Option<Workspace>,
}

impl<'a> PooledWorkspace<'a> {
    fn new(pool: &'a WorkspacePool, workspace: Workspace) -> Self {
        Self {
            pool,
            workspace: Some(workspace),
        }
    }

    pub(crate) fn get(&mut self) -> &mut Workspace {
        // Only `drop` takes the workspace out.
        let retention = self.pool.retention;
        self.workspace
            .get_or_insert_with(|| Workspace::new(retention))
    }
}

impl Drop for PooledWorkspace<'_> {
    fn drop(&mut self) {
        if let Some(mut workspace) = self.workspace.take() {
            // Release a long search's storage now, not when the workspace is next used,
            // and outside the pool's lock.
            workspace.trim();
            self.pool.lock().idle.push(workspace);
            self.pool.returned.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn reuses_workspaces_up_to_the_limit() {
        let pool = WorkspacePool::new(2, Retention::default());
        {
            let _a = pool.take();
            let _b = pool.take();
            assert_eq!(pool.created(), 2);
            assert_eq!(pool.idle(), 0);
        }
        assert_eq!(pool.idle(), 2);
        for _ in 0..10 {
            let mut workspace = pool.take();
            let _ = workspace.get();
        }
        assert_eq!(pool.created(), 2);
    }

    #[test]
    fn waits_when_every_workspace_is_in_use() {
        let pool = Arc::new(WorkspacePool::new(2, Retention::default()));
        let busy = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (pool, busy, most) = (pool.clone(), busy.clone(), most.clone());
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let _workspace = pool.take();
                        let now = busy.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_micros(200));
                        busy.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(most.load(Ordering::SeqCst) <= 2);
        assert_eq!(pool.created(), 2);
        assert_eq!(pool.idle(), 2);
    }

    #[test]
    fn a_panicking_request_gives_its_workspace_back() {
        let pool = Arc::new(WorkspacePool::new(1, Retention::default()));
        let cloned = pool.clone();
        let result = std::thread::spawn(move || {
            let _workspace = cloned.take();
            panic!("a request failed");
        })
        .join();
        assert!(result.is_err());
        assert_eq!(pool.idle(), 1);
        let _workspace = pool.take();
        assert_eq!(pool.created(), 1);
    }
}
