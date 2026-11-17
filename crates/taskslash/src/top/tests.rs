use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use taskslash_core::estimate::estimate;
use taskslash_core::protocol::Kind;
use taskslash_core::sampler::MachineSnapshot;

fn machine() -> MachineSnapshot {
    let calm = r#"{"some_avg10":0.0,"some_avg60":0.0,"full_avg10":0.0,"full_avg60":0.0}"#;
    let io = r#"{"some_avg10":1.25,"some_avg60":0.0,"full_avg10":0.5,"full_avg60":0.0}"#;
    serde_json::from_str(&format!(
        r#"{{"sampled_at":"2026-11-17T00:00:00.000Z",
"cpu":{{"threads":16,"threads_online":16,"busy":0.42,"pressure":{calm}}},
"memory":{{"total_mib":65536,"available_mib":49152,"swap_total_mib":0,"swap_free_mib":0,"pressure":{calm}}},
"io":{{"pressure":{io},"disks":[]}},
"mounts":[],
"load":[3.5,2.0,1.0]}}"#
    ))
    .unwrap()
}

fn running(job: u64, key: &str) -> JobStatus {
    JobStatus {
        job,
        key: key.into(),
        kind: Kind::Test,
        class: Class::Normal,
        submitter: "/home/me/dev/repo.wt-1".into(),
        started_at: "2026-11-17T00:00:00.000Z".into(),
        cpu_threads: None,
        mem_mib: None,
        estimate: estimate(&[], Kind::Test),
        log_path: format!("/logs/{job}.log"),
    }
}

fn waiting(job: u64, key: &str, position: usize) -> WaitingStatus {
    WaitingStatus {
        job,
        key: key.into(),
        kind: Kind::Build,
        class: Class::Background,
        effective_class: None,
        submitter: "/home/me/dev/repo.wt-2".into(),
        waited_s: 65.0,
        position,
        reason: "cpu pressure 90% over the 40% gate".into(),
        why: None,
        estimate: estimate(&[], Kind::Build),
    }
}

fn status(jobs: Vec<JobStatus>, waiting: Vec<WaitingStatus>) -> Status {
    Status {
        machine: machine(),
        jobs,
        waiting,
    }
}

fn queued(job: u64, key: &str) -> JobEvent {
    JobEvent::Queued {
        job,
        key: key.into(),
        kind: Kind::Command,
        class: Class::Interactive,
        submitter: "/home/me/dev/other".into(),
        cwd: "/home/me/dev/other".into(),
        argv: vec!["true".into()],
        at: "2026-11-17T00:00:00.000Z".into(),
    }
}

fn finished(job: u64, key: &str, log: Option<&str>) -> JobEvent {
    JobEvent::Finished {
        job,
        key: key.into(),
        exit_code: log.map(|_| 0),
        wall_ms: 1500,
        cached: None,
        moon_hash: None,
        run: None,
        log: log.map(str::to_owned),
        at: "2026-11-17T00:00:01.000Z".into(),
    }
}

fn jobs(app: &App) -> Vec<u64> {
    app.rows().iter().map(Row::job).collect()
}

#[test]
fn the_list_is_running_then_waiting_in_queue_order_then_finished_newest_first() {
    let mut app = App::new();
    app.event(queued(1, "first"));
    app.event(queued(2, "second"));
    app.status(status(
        vec![running(7, "gate")],
        vec![waiting(9, "late", 2), waiting(8, "early", 1)],
    ));
    app.event(finished(1, "first", Some("/logs/1.log")));
    app.event(finished(2, "second", None));

    assert_eq!(jobs(&app), [7, 8, 9, 2, 1]);
    let rows = app.rows();
    assert!(matches!(rows[0], Row::Running(_)));
    assert!(matches!(rows[1], Row::Waiting(_)));
    // The finished row is filled from what `queued` said.
    let Row::Recent(second) = rows[3] else {
        panic!("{rows:?}")
    };
    assert_eq!(second.class, Some(Class::Interactive));
    assert_eq!(second.submitter.as_deref(), Some("/home/me/dev/other"));
    let text = row_text(rows[3]);
    assert!(text.contains("never started"), "{text}");
    assert!(text.contains("interactive other  second"), "{text}");
    assert!(row_text(rows[1]).contains("waiting 1st 1m05s"));
}

#[test]
fn a_job_that_finished_is_listed_once_even_while_the_poll_still_runs_it() {
    let mut app = App::new();
    app.status(status(vec![running(7, "gate")], vec![]));
    app.event(finished(7, "gate", Some("/logs/7.log")));
    assert_eq!(jobs(&app), [7]);
    assert!(matches!(app.rows()[0], Row::Recent(r) if r.class == Some(Class::Normal)));
}

#[test]
fn the_selection_follows_its_job_number_across_a_reordering_refresh() {
    let mut app = App::new();
    app.status(status(
        vec![running(1, "a")],
        vec![waiting(2, "b", 1), waiting(3, "c", 2)],
    ));
    app.move_selection(2);
    assert_eq!(app.selected(), Some(3));
    // Job 3 moved up the queue, and job 1 finished.
    app.status(status(vec![], vec![waiting(3, "c", 1), waiting(2, "b", 2)]));
    assert_eq!(app.selected(), Some(3));
    assert_eq!(jobs(&app), [3, 2]);
    app.move_selection(1);
    assert_eq!(app.selected(), Some(2));
    app.move_selection(5);
    assert_eq!(app.selected(), Some(2), "stays on the last row");
}

#[test]
fn a_selected_job_that_leaves_the_list_hands_the_selection_to_its_place() {
    let mut app = App::new();
    app.status(status(vec![], vec![waiting(2, "b", 1), waiting(3, "c", 2)]));
    app.move_selection(1);
    app.status(status(vec![], vec![waiting(2, "b", 1)]));
    assert_eq!(app.selected(), Some(2));
}

#[test]
fn appended_log_bytes_come_out_stripped_and_only_new_lines_are_added() {
    let mut app = App::new();
    app.log_reset("/logs/1.log", b"\x1b[32mok\x1b[0m one\nhalf \x1b[3");
    assert_eq!(app.log_lines(), ["ok one", "half "]);
    app.log_append(b"1mway\x1b[0m\n\x1b]0;title\x07two\tthree\r\nbar 10%\rbar 100%\n\x07");
    assert_eq!(
        app.log_lines(),
        ["ok one", "half way", "two    three", "bar 100%"]
    );
    app.log_append(b"");
    assert_eq!(app.log_lines().len(), 4);
}

#[test]
fn clean_removes_escape_sequences_and_control_characters() {
    assert_eq!(clean(b"\x1b[2J\x1b[1;1Hhome"), "home");
    assert_eq!(clean(b"\x1b(Bplain\x1b7\x1b8"), "plain");
    assert_eq!(clean(b"\x1bP1;2|data\x1b\\after"), "after");
    assert_eq!(clean("caf\u{e9} \u{9b}x".as_bytes()), "caf\u{e9} x");
    assert_eq!(clean(b"a\x08b\x00c\x7f"), "abc");
}

#[test]
fn a_dropped_event_sets_the_notice() {
    let mut app = App::new();
    app.event(JobEvent::Dropped { count: 3 });
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("missed 3 job events")
    );
}

#[test]
fn the_pane_shows_a_log_a_reason_or_never_started() {
    let mut app = App::new();
    app.status(status(vec![running(1, "a")], vec![waiting(2, "b", 1)]));
    app.event(finished(3, "c", None));
    assert_eq!(app.pane(), Pane::Log("/logs/1.log"));
    app.move_selection(1);
    assert_eq!(
        app.pane(),
        Pane::Waiting("cpu pressure 90% over the 40% gate")
    );
    assert_eq!(app.wanted_log(), None);
    app.move_selection(1);
    assert_eq!(app.pane(), Pane::NeverStarted);
}

fn screen(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(app, frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn the_screen_shows_the_strip_a_waiting_reason_and_the_pane_title() {
    let mut app = App::new();
    app.status(status(
        vec![running(1, "gate")],
        vec![waiting(2, "slow", 1)],
    ));
    app.move_selection(1);
    let text = screen(&app, 140, 12);
    assert!(
        text.contains("cpu 42% busy on 16 threads, pressure 0.00   load 3.50 2.00 1.00"),
        "{text}"
    );
    assert!(
        text.contains(
            "mem 16.0 of 64.0 GiB used, 75% available, pressure 0.00   io some 1.25 full 0.50"
        ),
        "{text}"
    );
    assert!(text.contains("job 2 slow: waiting"), "{text}");
    assert!(
        text.contains("waiting: cpu pressure 90% over the 40% gate"),
        "{text}"
    );
    assert!(text.contains("1 running"), "{text}");

    app.move_selection(-1);
    app.log_reset("/logs/1.log", b"\x1b[1mcompiling\x1b[0m\nline two\n");
    let text = screen(&app, 140, 12);
    assert!(text.contains("job 1 gate: running"), "{text}");
    assert!(text.contains("compiling"), "{text}");
    assert!(text.contains("line two"), "{text}");
}

#[test]
fn a_log_tail_reads_the_end_then_only_what_is_appended() {
    use super::shell::LogTail;
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("tktop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("job.log");
    let mut tail = LogTail::new(path.to_string_lossy().into_owned());
    let missing = tail.read().unwrap_err();
    assert!(missing.starts_with("cannot read"), "{missing}");

    let mut file = std::fs::File::create(&path).unwrap();
    // More than the tail: the read starts at the first whole line inside it.
    let long = "x".repeat(100);
    for n in 0..1000 {
        writeln!(file, "{n:04} {long}").unwrap();
    }
    let (fresh, bytes) = tail.read().unwrap();
    assert!(fresh);
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text[..5].ends_with(" ") && text[..4].bytes().all(|b| b.is_ascii_digit()),
        "{}",
        &text[..20]
    );
    assert!(text.ends_with(&format!("0999 {long}\n")));
    assert!(text.len() <= 64 * 1024);

    write!(file, "new\npart").unwrap();
    assert_eq!(tail.read().unwrap(), (false, b"new\npart".to_vec()));
    assert_eq!(tail.read().unwrap(), (false, Vec::new()));

    std::fs::write(&path, "short\n").unwrap();
    assert_eq!(tail.read().unwrap(), (true, b"short\n".to_vec()));
    std::fs::remove_dir_all(&dir).unwrap();
}
