//! `taskslash daemon`: hands out jobs, measures them, and records a row for each when it ends. One
//! thread per connection; a connection is one job or one query. One more thread samples the
//! machine once a second, and `status` answers from its latest snapshot.

use crate::Error;
use crate::estimate::estimate;
use crate::launch;
use crate::protocol::{ClientMessage, DaemonMessage, Kind, Launch, Status, receive, send};
use crate::sampler::{MachineSampler, MachineSnapshot, ProcSampler};
use crate::scheduler::{Scheduler, lock};
use crate::store::Store;
use std::fs::{self, DirBuilder};
use std::io::BufReader;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

/// How often the machine is sampled.
const SAMPLE_EVERY: Duration = Duration::from_secs(1);

mod job;

/// What every connection shares. Lock order: `scheduler` before `store`, never the other way.
struct Daemon {
    launch: Launch,
    store: Mutex<Store>,
    next_job: AtomicU64,
    machine: Mutex<MachineSnapshot>,
    scheduler: Mutex<Scheduler>,
}

/// Binds the socket at `path`, opens the database at `db`, and serves until the process is killed.
pub fn serve(path: &Path, db: &Path) -> Result<(), Error> {
    let store = Store::open(db)?;
    // Asked before binding, so a daemon that accepts has already logged its launch path.
    let launch = launch::probe();
    let listener = bind(path)?;
    if launch == Launch::Plain {
        eprintln!("taskslash: no systemd user session, jobs run unmeasured");
    }
    eprintln!(
        "taskslash daemon listening on {} (launch: {}, database: {})",
        path.display(),
        launch.as_str(),
        db.display()
    );
    // The first snapshot is taken before the first connection, so `status` always has one.
    let mut machine = ProcSampler::new();
    let daemon = Arc::new(Daemon {
        launch,
        store: Mutex::new(store),
        next_job: AtomicU64::new(1),
        machine: Mutex::new(machine.sample()),
        scheduler: Mutex::new(Scheduler::default()),
    });
    let sampling = Arc::clone(&daemon);
    thread::spawn(move || sampling.sample_forever(machine));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let daemon = Arc::clone(&daemon);
                thread::spawn(move || daemon.serve_connection(stream));
            }
            Err(e) => eprintln!("taskslash: accept failed: {e}"),
        }
    }
    Ok(())
}

/// Binds `path`, creating its parent with mode 0700 (the socket's permissions are the
/// authentication). A socket file nobody answers on is stale and is replaced; anything else at
/// `path` is left alone.
fn bind(path: &Path) -> Result<UnixListener, Error> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| Error::new(format!("cannot create {}: {e}", parent.display())))?;
    }
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(Error::new(format!(
                "cannot bind {}: not a socket",
                path.display()
            )));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(Error::new(format!(
                "a daemon is already running on {}",
                path.display()
            )));
        }
        fs::remove_file(path)
            .map_err(|e| Error::new(format!("cannot remove stale {}: {e}", path.display())))?;
    }
    UnixListener::bind(path).map_err(|e| Error::new(format!("cannot bind {}: {e}", path.display())))
}

impl Daemon {
    fn serve_connection(&self, stream: UnixStream) {
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(read_half);
        let mut writer = stream;
        match receive(&mut reader) {
            // A connection that closes without asking is a probe (the stale check, a readiness wait).
            Ok(None) => {}
            Ok(Some(ClientMessage::Submit {
                name,
                kind,
                argv,
                cwd,
            })) if !argv.is_empty() => {
                self.serve_job(&mut reader, &mut writer, name.as_deref(), kind, &argv, &cwd);
            }
            Ok(Some(ClientMessage::Submit { .. })) => {
                refuse(&mut writer, "submit needs a non-empty argv");
            }
            Ok(Some(ClientMessage::History { key })) => {
                let reply = match self.history(&key) {
                    Ok(reply) => reply,
                    Err(e) => DaemonMessage::Error {
                        message: e.to_string(),
                    },
                };
                let _ = send(&mut writer, &reply);
            }
            Ok(Some(ClientMessage::Status)) => {
                let _ = send(&mut writer, &DaemonMessage::Status(Box::new(self.status())));
            }
            Ok(Some(ClientMessage::Exited { .. })) => {
                refuse(
                    &mut writer,
                    "expected submit, history or status, got exited",
                );
            }
            Err(e) => refuse(&mut writer, &format!("bad request: {e}")),
        }
    }

    /// The key's runs, and its estimate under the newest run's kind (`command` with no runs).
    fn history(&self, key: &str) -> Result<DaemonMessage, Error> {
        let store = self.store();
        let runs = store.history(key)?;
        let kind = runs.first().map_or(Kind::Command, |run| run.kind);
        let estimate = estimate(&store.costs(key)?, kind);
        Ok(DaemonMessage::Runs { estimate, runs })
    }

    fn admit(&self) {
        self.admit_locked(&mut self.scheduler());
    }

    /// Admits with the scheduler already locked; reads the store for whether a key is seen. A key
    /// whose history cannot be read counts as never seen.
    fn admit_locked(&self, scheduler: &mut Scheduler) {
        scheduler.admit(|key| {
            self.store().seen(key).unwrap_or_else(|e| {
                eprintln!("taskslash: {key} counted as never seen: {e}");
                false
            })
        });
    }

    fn scheduler(&self) -> MutexGuard<'_, Scheduler> {
        lock(&self.scheduler)
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }

    /// Samples the machine on every whole `SAMPLE_EVERY` from now, skipping a deadline already
    /// missed, so the period does not drift by the time a sample takes.
    fn sample_forever(&self, mut sampler: impl MachineSampler) {
        let mut next = Instant::now();
        loop {
            next += SAMPLE_EVERY;
            let now = Instant::now();
            while next <= now {
                next += SAMPLE_EVERY;
            }
            thread::sleep(next - now);
            let snapshot = sampler.sample();
            {
                let mut scheduler = self.scheduler();
                scheduler.sample(&snapshot);
                self.admit_locked(&mut scheduler);
            }
            *lock(&self.machine) = snapshot;
        }
    }

    fn status(&self) -> Status {
        let machine = lock(&self.machine).clone();
        let (jobs, waiting) = self.scheduler().status();
        Status {
            machine,
            jobs,
            waiting,
        }
    }
}

fn refuse(writer: &mut UnixStream, message: &str) {
    let _ = send(
        writer,
        &DaemonMessage::Error {
            message: message.to_owned(),
        },
    );
}
