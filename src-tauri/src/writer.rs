// AYGENT — Single-writer actor (Atlas C6, M1.1).
//
// THE GUARANTEE: exactly ONE thread ever mutates the database. Every write is a
// closure sent over a channel to that thread, which runs it against the sole
// write connection and returns the result. WAL lets any number of reader
// connections run concurrently without ever blocking or corrupting the writer.
//
// WHY AN ACTOR AND NOT A Mutex<Connection>:
//   - A Mutex serializes writes too, but interleaves them with readers holding
//     the lock and invites deadlocks across Tauri's async command threads. An
//     actor gives a single, well-defined mutation point (Atlas C6's intent) and
//     keeps `rusqlite::Connection` (which is !Sync) off every other thread.
//   - It's the natural serialization point that C5 (single-writer-per-note) and
//     C4 (save point quiesce) also want: one place, one order.
//
// USAGE:
//   let db = Db::start(app_data)?;            // spawns the writer thread
//   db.write(|c| { c.execute(...); Ok(()) })?; // runs on the writer thread
//   let conn = db.reader()?;                    // fresh concurrent read conn

use crate::db;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;

/// A unit of work handed to the writer thread. Boxed so any closure returning a
/// String-error result can be queued. `Send` because it crosses the channel.
type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// Handle to the state database. Cheap to clone (just the sender + app_data);
/// held in Tauri state and shared across all commands.
///
/// CONFIG RELOCATION (2026-07-31): `app_data` is behind a Mutex + the writer
/// thread can be RE-POINTED live (repoint) when onboarding sets the root — so we
/// swap the DB from app-data to <root>/.aygent WITHOUT restarting the process
/// (app.restart() from inside a command future aborts — that was the crash).
#[derive(Clone)]
pub struct Db {
    tx: Sender<Job>,
    app_data: std::sync::Arc<std::sync::Mutex<PathBuf>>,
    // Set once onboarding re-points; the writer thread swaps its connection to
    // the new path on the next job. Held as a shared cell the writer reads.
    repoint: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
}

impl Db {
    /// Open the DB, run migrations, and spawn the single writer thread. The
    /// write connection lives ONLY on that thread and is never shared.
    pub fn start(app_data: PathBuf) -> Result<Self, String> {
        // Open once on the caller to surface migration errors synchronously,
        // then MOVE that connection onto the writer thread (never shared).
        let mut conn = db::open(&app_data)?;
        let (tx, rx) = mpsc::channel::<Job>();

        let repoint: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let repoint_w = repoint.clone();

        thread::Builder::new()
            .name("aygent-db-writer".into())
            .spawn(move || {
                // The connection lives and dies on this thread. When the last
                // Db handle drops, the channel closes, the loop ends, the
                // connection closes cleanly.
                for job in rx {
                    // RELOCATION: before each job, check whether onboarding asked
                    // to re-point the DB. If so, checkpoint the OLD wal, open the
                    // NEW path, and swap — no process restart. Best-effort: a
                    // failed reopen keeps the current connection so we never lose
                    // the writer.
                    let pending = repoint_w.lock().unwrap().take();
                    if let Some(new_path) = pending {
                        let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
                        match db::open(&new_path) {
                            Ok(new_conn) => {
                                conn = new_conn;
                                eprintln!("[aygent] db re-pointed to {}", new_path.display());
                            }
                            Err(e) => eprintln!("[aygent] db repoint FAILED (keeping current): {e}"),
                        }
                    }
                    job(&mut conn);
                }
            })
            .map_err(|e| format!("spawn writer thread: {e}"))?;

        Ok(Db {
            tx,
            app_data: std::sync::Arc::new(std::sync::Mutex::new(app_data)),
            repoint,
        })
    }

    /// RELOCATION: re-point the live DB to a new directory (onboarding set the
    /// root). Queues the swap for the writer thread and updates app_data (which
    /// readers use). The next write — e.g. agents_create — lands in the new DB.
    /// We run a no-op write immediately so the swap happens before the caller's
    /// next real write, and the new DB's migrations run.
    pub fn repoint(&self, new_dir: PathBuf) -> Result<(), String> {
        *self.repoint.lock().unwrap() = Some(new_dir.clone());
        *self.app_data.lock().unwrap() = new_dir;
        // Force the writer to process the pending repoint now (empty job).
        self.write(|_c| Ok(()))
    }

    /// Run a write on the writer thread and wait for its result. The closure
    /// gets the sole write connection; returning Err rolls back if it used a txn.
    /// Blocking (waits on a oneshot) — Tauri commands are already on a worker
    /// thread, so this never blocks the UI.
    pub fn write<T, F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let (rtx, rrx) = mpsc::channel::<Result<T, String>>();
        let job: Job = Box::new(move |conn| {
            let out = f(conn);
            // If the receiver is gone (caller dropped), we just drop the result.
            let _ = rtx.send(out);
        });
        self.tx
            .send(job)
            .map_err(|_| "db writer thread is gone".to_string())?;
        rrx.recv()
            .map_err(|_| "db writer dropped the job".to_string())?
    }

    /// A fresh reader connection. WAL => concurrent with the writer. Callers
    /// that only SELECT use this; they never touch the writer thread.
    pub fn reader(&self) -> Result<Connection, String> {
        let dir = self.app_data.lock().unwrap().clone();
        db::open_reader(&dir)
    }

    /// The app-data dir (used for one-time JSON->SQLite migration source paths).
    pub fn app_data(&self) -> PathBuf {
        self.app_data.lock().unwrap().clone()
    }
}
