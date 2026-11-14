//! Bounded pools of workspaces: Thor's path search workspaces, and Meili's map matching ones.
//!
//! A Thor [`Workspace`] holds a search's labels, queues, and edge status, and keeps their storage
//! between requests so that searches don't allocate as they go;
//! a Meili workspace holds the candidate grids (a cache mainline keeps in its matcher factory)
//! and the match's search state.
//! Requests take one from the pool and give it back when they are done.
//! At most `limit` workspaces are ever made (lazily, as concurrent requests need them);
//! when all are in use, a request waits for one to come back.
//! So a pool also bounds how many searches (or matches) run at once, and their memory.
//! A path search workspace is trimmed to its [`Retention`] when it comes back,
//! so an idle pool holds at most that much search storage per workspace.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use valinor_meili::Workspace as MeiliWorkspace;
use valinor_thor::{Retention, Workspace};

/// A workspace a pool can make and take back.
pub(crate) trait Reusable: Sized {
    /// What a new workspace is made with.
    type Settings: Copy;

    /// A new workspace.
    fn make(settings: Self::Settings) -> Self;

    /// Gives back what the workspace shouldn't keep while it is idle.
    fn release(&mut self);
}

impl Reusable for Workspace {
    type Settings = Retention;

    fn make(retention: Retention) -> Self {
        Self::new(retention)
    }

    fn release(&mut self) {
        // Release a long search's storage now, not when the workspace is next used.
        self.trim();
    }
}

impl Reusable for MeiliWorkspace {
    type Settings = ();

    fn make((): ()) -> Self {
        Self::new()
    }

    /// The candidate grids stay, as mainline's matcher factory keeps them
    /// (Meili clears them before a match once there are more than `cache_size`).
    fn release(&mut self) {}
}

struct State<W> {
    idle: Vec<W>,
    created: usize,
}

pub(crate) struct WorkspacePool<W: Reusable = Workspace> {
    state: Mutex<State<W>>,
    returned: Condvar,
    limit: usize,
    settings: W::Settings,
}

impl<W: Reusable> WorkspacePool<W> {
    pub(crate) fn new(limit: usize, settings: W::Settings) -> Self {
        Self {
            state: Mutex::new(State {
                idle: Vec::with_capacity(limit),
                created: 0,
            }),
            returned: Condvar::new(),
            limit,
            settings,
        }
    }

    /// The lock never guards a half-made change (nothing in it can panic midway),
    /// so a poisoned lock is still consistent.
    fn lock(&self) -> MutexGuard<'_, State<W>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes an idle workspace, makes one if fewer than the limit exist,
    /// or waits for one to be returned.
    pub(crate) fn take(&self) -> PooledWorkspace<'_, W> {
        let mut state = self.lock();
        loop {
            if let Some(workspace) = state.idle.pop() {
                return PooledWorkspace::new(self, workspace);
            }
            if state.created < self.limit {
                state.created += 1;
                // Don't hold the lock while the workspace allocates.
                drop(state);
                return PooledWorkspace::new(self, W::make(self.settings));
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
pub(crate) struct PooledWorkspace<'a, W: Reusable = Workspace> {
    pool: &'a WorkspacePool<W>,
    workspace: Option<W>,
}

impl<'a, W: Reusable> PooledWorkspace<'a, W> {
    fn new(pool: &'a WorkspacePool<W>, workspace: W) -> Self {
        Self {
            pool,
            workspace: Some(workspace),
        }
    }

    pub(crate) fn get(&mut self) -> &mut W {
        // Only `drop` takes the workspace out.
        let settings = self.pool.settings;
        self.workspace.get_or_insert_with(|| W::make(settings))
    }
}

impl<W: Reusable> Drop for PooledWorkspace<'_, W> {
    fn drop(&mut self) {
        if let Some(mut workspace) = self.workspace.take() {
            // Outside the pool's lock.
            workspace.release();
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
        let pool = WorkspacePool::<Workspace>::new(2, Retention::default());
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
        let pool = Arc::new(WorkspacePool::<Workspace>::new(2, Retention::default()));
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
        let pool = Arc::new(WorkspacePool::<MeiliWorkspace>::new(1, ()));
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
