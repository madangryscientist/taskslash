//! `taskslash daemon`: hands out jobs, measures them, and records a row for each when it ends. One
//! thread per connection; a connection is one job or one query. One more thread samples the
//! machine once a second, and `status` answers from its latest snapshot.

use crate::Error;
use crate::launch;
use crate::measure::{self, Cost, LiveReading, Sampler};
use crate::procfs;
use crate::protocol::{ClientMessage, DaemonMessage, JobStatus, Launch, Status, receive, send};
use crate::sampler::{MachineSampler, MachineSnapshot, ProcSampler};
use crate::store::{self, Finished, Store};
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder};
use std::io::BufReader;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// How often the machine is sampled.
const SAMPLE_EVERY: Duration = Duration::from_secs(1);

/// What every connection shares.
struct Daemon {
    launch: Launch,
    store: Mutex<Store>,
    next_job: AtomicU64,
    machine: Mutex<MachineSnapshot>,
    jobs: Mutex<BTreeMap<u64, Running>>,
}

/// A job between `start` and its end.
struct Running {
    key: String,
    started_at: String,
    /// `None` on the plain launch path, where nothing measures the job.
    live: Option<LiveReading>,
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
        jobs: Mutex::new(BTreeMap::new()),
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
            Ok(Some(ClientMessage::Submit { name, argv, cwd })) if !argv.is_empty() => {
                self.serve_job(&mut reader, &mut writer, name.as_deref(), &argv, &cwd);
            }
            Ok(Some(ClientMessage::Submit { .. })) => {
                refuse(&mut writer, "submit needs a non-empty argv");
            }
            Ok(Some(ClientMessage::History { key })) => {
                let reply = match self.store().history(&key) {
                    Ok(runs) => DaemonMessage::Runs { runs },
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

    /// A job: `start`, sampling while it runs, then a row when `exited` arrives (and `recorded`) or
    /// when the connection drops (no exit code).
    fn serve_job(
        &self,
        reader: &mut BufReader<UnixStream>,
        writer: &mut UnixStream,
        name: Option<&str>,
        argv: &[String],
        cwd: &str,
    ) {
        let job = self.next_job.fetch_add(1, Ordering::Relaxed);
        // The daemon's pid keeps unit names apart across restarts and daemons on other sockets.
        let unit = format!("taskslash-{}-{job}", process::id());
        eprintln!("job {job} started: {}", argv.join(" "));
        let started_at = SystemTime::now();
        let clock = Instant::now();
        let start = DaemonMessage::Start {
            job,
            unit: unit.clone(),
            launch: self.launch,
        };
        let sampler = match self.launch {
            Launch::Scope => measure::cgroup_dir(&unit).map(Sampler::start),
            Launch::Plain => None,
        };
        let key = store::key(name, argv);
        lock(&self.jobs).insert(
            job,
            Running {
                key: key.clone(),
                started_at: procfs::rfc3339(started_at),
                live: sampler.as_ref().map(Sampler::live),
            },
        );
        let exit_code = if send(writer, &start).is_ok() {
            match receive(reader) {
                Ok(Some(ClientMessage::Exited { job: id, code })) if id == job => Some(code),
                _ => None,
            }
        } else {
            None
        };
        let wall_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        lock(&self.jobs).remove(&job);
        let cost = sampler.map(Sampler::finish).unwrap_or_else(Cost::default);
        match exit_code {
            Some(code) => eprintln!("job {job} exited {code}"),
            None => eprintln!("job {job} dropped"),
        }
        let finished = Finished {
            key: &key,
            argv,
            cwd,
            launch: self.launch,
            started_at,
            wall_ms,
            cost,
            exit_code,
        };
        match self.store().record(&finished) {
            // The client may already be gone; the row is written either way.
            Ok(_) if exit_code.is_some() => {
                let _ = send(writer, &DaemonMessage::Recorded { job });
            }
            Ok(_) => {}
            Err(e) => eprintln!("taskslash: job {job} not recorded: {e}"),
        }
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
            *lock(&self.machine) = snapshot;
        }
    }

    fn status(&self) -> Status {
        let machine = lock(&self.machine).clone();
        let jobs = lock(&self.jobs)
            .iter()
            .map(|(&job, running)| {
                let live = running.live.as_ref().and_then(|live| *lock(live));
                JobStatus {
                    job,
                    key: running.key.clone(),
                    started_at: running.started_at.clone(),
                    cpu_threads: live.map(|l| (l.cpu_threads * 100.0).round() / 100.0),
                    mem_mib: live.map(|l| l.mem_mib),
                }
            })
            .collect();
        Status { machine, jobs }
    }
}

/// A panic while holding a lock leaves that connection, not the shared state, broken.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn refuse(writer: &mut UnixStream, message: &str) {
    let _ = send(
        writer,
        &DaemonMessage::Error {
            message: message.to_owned(),
        },
    );
}
