//! Running a job's command with its output copied into the job's log as well as to the caller.
//!
//! `taskslash run` runs the command on a pseudo-terminal when the caller's stdout is a terminal (so
//! colours and progress bars stay), its window size following the caller's and the caller's stdin
//! forwarded to it when stdin is a terminal too; otherwise on two pipes, stdout and stderr each
//! copied to the caller's matching stream. Either way the command is in a process group of its
//! own, and SIGINT, SIGTERM and SIGHUP sent to the client are forwarded to that group. The caller's
//! terminal is put back as it was when the command ends, and when it is stopped (Ctrl-Z).

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// How long the copy loop waits for more output once the command has exited, and its poll period.
const DRAIN: Duration = Duration::from_millis(100);
/// The signals the client forwards to the command's process group.
const FORWARDED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Opens (creating it, mode 0600, and its directory, mode 0700) the log at `path` for appending.
/// A log that cannot be opened costs a warning, not the job.
pub(crate) fn open_log(path: &Path) -> Option<File> {
    let made = path.parent().map_or(Ok(()), |dir| {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)
    });
    let opened = made.and_then(|()| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
    });
    match opened {
        Ok(file) => Some(file),
        Err(e) => {
            eprintln!("taskslash: warning: no log at {}: {e}", path.display());
            None
        }
    }
}

/// One file descriptor the copy loop reads.
pub(crate) struct Source {
    pub fd: OwnedFd,
    /// Output, copied into the log too; forwarded input is not.
    pub output: bool,
}

/// Copies from `sources` until every output is at its end, or `done` is set and nothing arrived
/// for `DRAIN` (a process the command left behind may hold an output open). Each chunk of output
/// goes into `log`, then every chunk to `deliver(index, chunk)`, which returns false when its
/// destination is gone: that source is then closed, so a command writing to it gets SIGPIPE (or
/// SIGHUP on a terminal) as it would have with no client in between.
pub(crate) fn pump(
    sources: Vec<Source>,
    mut log: Option<&mut File>,
    done: &AtomicBool,
    mut deliver: impl FnMut(usize, &[u8]) -> bool,
) {
    let mut open: Vec<Option<Source>> = sources.into_iter().map(Some).collect();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let finished = done.load(Ordering::Acquire);
        let outputs = open.iter().flatten().any(|s| s.output);
        if finished && !outputs {
            break;
        }
        let mut fds: Vec<libc::pollfd> = Vec::new();
        let mut index = Vec::new();
        for (i, source) in open.iter().enumerate() {
            if let Some(source) = source {
                fds.push(libc::pollfd {
                    fd: source.fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
                index.push(i);
            }
        }
        let wait = i32::try_from(DRAIN.as_millis()).unwrap_or(100);
        // SAFETY: `fds` is a valid array of `fds.len()` pollfds for the duration of the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 {
            if finished {
                break;
            }
            continue;
        }
        for (fd, &i) in fds.iter().zip(&index) {
            if fd.revents == 0 {
                continue;
            }
            let Some(source) = &open[i] else { continue };
            // SAFETY: `buf` is valid for `buf.len()` bytes and `fd` is open.
            let got = unsafe { libc::read(fd.fd, buf.as_mut_ptr().cast(), buf.len()) };
            let keep = match usize::try_from(got) {
                Ok(0) => false,
                Ok(n) => {
                    let chunk = &buf[..n];
                    if source.output
                        && let Some(log) = log.as_mut()
                    {
                        // A full disk loses the log, never the caller's copy or the job.
                        let _ = log.write_all(chunk);
                    }
                    deliver(i, chunk)
                }
                // EIO is a terminal's end, once the command and everything it started are gone.
                Err(_) => matches!(
                    io::Error::last_os_error().kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ),
            };
            if !keep {
                open[i] = None;
            }
        }
    }
}

/// Writes all of `bytes` to `fd`, unbuffered.
pub(crate) fn write_fd(fd: RawFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is valid for `bytes.len()` bytes.
        let wrote = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        match usize::try_from(wrote) {
            Ok(n) => bytes = &bytes[n..],
            Err(_) => {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}

fn is_tty(fd: RawFd) -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

fn errno<T>(result: libc::c_int, value: T) -> io::Result<T> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}

/// The signals the client keeps for itself while a command runs: the forwarded ones, SIGWINCH,
/// and SIGTTOU (so handing the terminal back and forth never stops the client).
fn held_signals() -> libc::sigset_t {
    // SAFETY: sigemptyset and sigaddset initialise and fill a local set.
    unsafe {
        let mut set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&raw mut set);
        for signal in FORWARDED {
            libc::sigaddset(&raw mut set, signal);
        }
        libc::sigaddset(&raw mut set, libc::SIGWINCH);
        libc::sigaddset(&raw mut set, libc::SIGTTOU);
        set
    }
}

/// Forwards the signals the client receives to the command's process group, once there is one
/// (a signal that comes first waits for it), and the caller's window size to the pseudo-terminal.
struct Forwarder {
    state: Mutex<(Option<libc::pid_t>, Vec<libc::c_int>)>,
    /// A copy of the pseudo-terminal's master of its own, so a late SIGWINCH never meets a closed fd.
    master: Option<OwnedFd>,
}

impl Forwarder {
    /// Blocks the held signals in this thread (every thread it starts after inherits that) and
    /// starts the thread that waits for them. It lives until the process exits.
    fn start(master: Option<OwnedFd>) -> Arc<Self> {
        let held = held_signals();
        // SAFETY: `held` is an initialised set; the old mask is not wanted.
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &raw const held, std::ptr::null_mut()) };
        let forwarder = Arc::new(Self {
            state: Mutex::new((None, Vec::new())),
            master,
        });
        let waiting = Arc::clone(&forwarder);
        thread::spawn(move || {
            let mut wanted = held;
            // SAFETY: removes one signal from a local, initialised set.
            unsafe { libc::sigdelset(&raw mut wanted, libc::SIGTTOU) };
            loop {
                let mut signal = 0;
                // SAFETY: `wanted` is initialised and blocked in every thread; `signal` is written.
                if unsafe { libc::sigwait(&raw const wanted, &raw mut signal) } != 0 {
                    continue;
                }
                if signal == libc::SIGWINCH {
                    waiting.resize();
                } else {
                    waiting.forward(signal);
                }
            }
        });
        forwarder
    }

    fn forward(&self, signal: libc::c_int) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.0 {
            // SAFETY: kill with a negative pid signals that process group; a gone group is ESRCH.
            Some(group) => unsafe {
                libc::kill(-group, signal);
            },
            None => state.1.push(signal),
        }
    }

    /// The command started as process group `group`: it gets any signal that came first.
    fn started(&self, group: libc::pid_t) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 = Some(group);
        for signal in state.1.drain(..) {
            // SAFETY: as in `forward`.
            unsafe { libc::kill(-group, signal) };
        }
    }

    fn resize(&self) {
        if let Some(master) = &self.master
            && let Some(size) = window_size(libc::STDOUT_FILENO)
        {
            // SAFETY: `size` is a valid winsize; the kernel tells the command with SIGWINCH.
            unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &raw const size) };
        }
    }
}

fn window_size(fd: RawFd) -> Option<libc::winsize> {
    // SAFETY: TIOCGWINSZ writes one winsize.
    unsafe {
        let mut size = std::mem::zeroed::<libc::winsize>();
        (libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut size) == 0).then_some(size)
    }
}

fn termios(fd: RawFd) -> Option<libc::termios> {
    // SAFETY: tcgetattr writes one termios.
    unsafe {
        let mut termios = std::mem::zeroed::<libc::termios>();
        (libc::tcgetattr(fd, &raw mut termios) == 0).then_some(termios)
    }
}

/// The caller's terminal as the client found it, and what it changes while the command runs:
/// raw mode on stdin (the pseudo-terminal path, so each key goes to the command's terminal), or
/// the terminal's foreground handed to the command's group (the pipe path with a terminal on stdin,
/// so the command may read it). `Drop` puts it back.
struct Caller {
    saved: Option<libc::termios>,
    raw: bool,
    handoff: bool,
    group: Option<libc::pid_t>,
}

impl Caller {
    /// Whether the client's group is the terminal on stdin's foreground.
    fn in_foreground() -> bool {
        // SAFETY: tcgetpgrp and getpgrp only read.
        is_tty(libc::STDIN_FILENO)
            && unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) == libc::getpgrp() }
    }

    /// Takes over the terminal: raw mode when `raw`, the foreground to `group` when `handoff`.
    fn take(&self) {
        if !Self::in_foreground() {
            return;
        }
        if self.raw
            && let Some(saved) = self.saved
        {
            let mut raw = saved;
            // SAFETY: cfmakeraw edits a local copy; tcsetattr applies it to stdin.
            unsafe {
                libc::cfmakeraw(&raw mut raw);
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSADRAIN, &raw const raw);
            }
        }
        if self.handoff
            && let Some(group) = self.group
        {
            // SAFETY: SIGTTOU is blocked, so this never stops the client.
            unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, group) };
        }
    }

    /// Puts the terminal back as it was found.
    fn give_back(&self) {
        // SAFETY: as in `take`; restoring a saved termios and the client's own group.
        unsafe {
            if self.handoff && self.group.is_some() {
                libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp());
            }
            if self.raw
                && let Some(saved) = self.saved
            {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSADRAIN, &raw const saved);
            }
        }
    }
}

impl Drop for Caller {
    fn drop(&mut self) {
        self.give_back();
    }
}

/// Runs `command` to its end with its output copied to the caller and into `log`, forwarding
/// signals to it and restoring the caller's terminal after. Returns how it ended; an `Err` means it
/// never started.
pub(crate) fn run(mut command: Command, mut log: Option<File>) -> io::Result<ExitStatus> {
    let pty = is_tty(libc::STDOUT_FILENO);
    let stdin_tty = is_tty(libc::STDIN_FILENO);
    let mut caller = Caller {
        saved: termios(libc::STDIN_FILENO),
        raw: pty && stdin_tty,
        handoff: !pty && Caller::in_foreground(),
        group: None,
    };
    let mut sources = Vec::new();
    // Where each source's bytes go: an fd of the caller's, or the pseudo-terminal.
    let mut sinks = Vec::new();
    let master = if pty {
        let (master, slave) = open_pty()?;
        command.stdout(Stdio::from(slave.try_clone()?));
        if stdin_tty {
            command.stdin(Stdio::from(slave.try_clone()?));
        }
        if is_tty(libc::STDERR_FILENO) {
            command.stderr(Stdio::from(slave));
        } else {
            command.stderr(Stdio::piped());
        }
        Some(master)
    } else {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        None
    };
    let handoff = caller.handoff;
    // SAFETY: the closure runs in the forked child before exec and only makes async-signal-safe
    // calls.
    unsafe {
        command.pre_exec(move || {
            if pty {
                errno(libc::setsid(), ())?;
                errno(libc::ioctl(libc::STDOUT_FILENO, libc::TIOCSCTTY, 0), ())?;
            } else {
                errno(libc::setpgid(0, 0), ())?;
                if handoff {
                    // SIGTTOU is still blocked, as it is in the client, so this cannot stop it.
                    libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpid());
                }
            }
            let mut none = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&raw mut none);
            errno(
                libc::pthread_sigmask(libc::SIG_SETMASK, &raw const none, std::ptr::null_mut()),
                (),
            )
        });
    }
    // The stdin copy writes to a master of its own, which outlives the copy loop's.
    let to_master = master.as_ref().map(OwnedFd::try_clone).transpose()?;
    let forwarder = Forwarder::start(master.as_ref().map(OwnedFd::try_clone).transpose()?);
    let mut child = command.spawn()?;
    // The command's copies of the terminal's other end are closed with it, so reading the master
    // ends when the command and what it started are gone.
    drop(command);
    let pid = libc::pid_t::try_from(child.id()).unwrap_or(0);
    forwarder.started(pid);
    caller.group = Some(pid);
    caller.take();

    if let Some(master) = master {
        // A client in the terminal's background would be stopped (SIGTTIN) for reading it.
        if let Some(to_master) = &to_master
            && stdin_tty
            && Caller::in_foreground()
        {
            // SAFETY: dup returns a new fd (or -1, checked) that only the source owns.
            let fd = unsafe { libc::dup(libc::STDIN_FILENO) };
            if fd >= 0 {
                sources.push(Source {
                    // SAFETY: as above.
                    fd: unsafe { OwnedFd::from_raw_fd(fd) },
                    output: false,
                });
                sinks.push(to_master.as_raw_fd());
            }
        }
        sinks.push(libc::STDOUT_FILENO);
        sources.push(Source {
            fd: master,
            output: true,
        });
    } else if let Some(stdout) = child.stdout.take() {
        sinks.push(libc::STDOUT_FILENO);
        sources.push(Source {
            fd: stdout.into(),
            output: true,
        });
    }
    if let Some(stderr) = child.stderr.take() {
        sinks.push(libc::STDERR_FILENO);
        sources.push(Source {
            fd: stderr.into(),
            output: true,
        });
    }

    let done = AtomicBool::new(false);
    let status = thread::scope(|scope| {
        let done = &done;
        let log = log.as_mut();
        let sinks = &sinks;
        scope.spawn(move || {
            pump(sources, log, done, |i, chunk| {
                write_fd(sinks[i], chunk).is_ok()
            });
        });
        let status = wait(pid, &caller);
        done.store(true, Ordering::Release);
        status
    });
    drop(caller);
    status
}

/// Waits for `pid` to end. When it stops (Ctrl-Z), the client gives the terminal back and stops
/// its own group the same way; when continued, it takes the terminal again and continues it.
fn wait(pid: libc::pid_t, caller: &Caller) -> io::Result<ExitStatus> {
    loop {
        let mut status = 0;
        // SAFETY: waits for our own child; `status` is written.
        let reaped = unsafe { libc::waitpid(pid, &raw mut status, libc::WUNTRACED) };
        if reaped < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if libc::WIFSTOPPED(status) {
            caller.give_back();
            // SAFETY: stops the client the way the command was stopped: its whole group (a
            // shell's job) under job control, else itself. It returns once continued.
            unsafe {
                if is_tty(libc::STDIN_FILENO) {
                    libc::kill(0, libc::SIGTSTP);
                } else {
                    libc::kill(libc::getpid(), libc::SIGSTOP);
                }
            }
            caller.take();
            // SAFETY: continues the command's group.
            unsafe { libc::kill(-pid, libc::SIGCONT) };
            continue;
        }
        return Ok(ExitStatus::from_raw(status));
    }
}

/// A pseudo-terminal with the caller's stdout's settings and window size: (master, slave), both
/// close-on-exec.
fn open_pty() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    let settings = termios(libc::STDOUT_FILENO);
    let size = window_size(libc::STDOUT_FILENO);
    // SAFETY: openpty writes two fds; the settings and size are valid or null.
    let opened = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            settings
                .as_ref()
                .map_or(std::ptr::null(), std::ptr::from_ref),
            size.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
        )
    };
    errno(opened, ())?;
    // SAFETY: openpty returned two open fds that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        // SAFETY: sets a flag on an fd we own.
        errno(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
            (),
        )?;
    }
    Ok((master, slave))
}
