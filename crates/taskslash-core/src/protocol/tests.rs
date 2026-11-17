//! The wire format.

use super::*;

#[test]
fn messages_round_trip_as_tagged_json_lines() {
    let submit = ClientMessage::Submit {
        name: Some("probe".into()),
        kind: Kind::Browser,
        argv: vec!["sh".into(), "-c".into(), "exit 3".into()],
        cwd: "/work".into(),
        cpu: None,
        mem_mib: None,
        class: Class::Flake,
        submitter: "/work".into(),
    };
    let start = DaemonMessage::Start {
        job: 3,
        unit: "taskslash-1-3".into(),
        launch: Launch::Scope,
        log: "/logs/taskslash-1-3.log".into(),
    };
    let history = ClientMessage::History {
        key: "probe".into(),
        limit: None,
        within: None,
    };
    let mut wire = Vec::new();
    send(&mut wire, &submit).unwrap();
    send(&mut wire, &start).unwrap();
    send(&mut wire, &history).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&wire),
        "{\"type\":\"submit\",\"name\":\"probe\",\"kind\":\"browser\",\"argv\":[\"sh\",\"-c\",\"exit 3\"],\"cwd\":\"/work\",\"class\":\"flake\",\"submitter\":\"/work\"}\n\
         {\"type\":\"start\",\"job\":3,\"unit\":\"taskslash-1-3\",\"launch\":\"scope\",\"log\":\"/logs/taskslash-1-3.log\"}\n\
         {\"type\":\"history\",\"key\":\"probe\"}\n"
    );
    let mut reader = wire.as_slice();
    assert_eq!(receive(&mut reader).unwrap(), Some(submit));
    assert_eq!(receive(&mut reader).unwrap(), Some(start));
    assert_eq!(receive(&mut reader).unwrap(), Some(history));
    assert_eq!(receive::<DaemonMessage>(&mut reader).unwrap(), None);
}

#[test]
fn subscribe_and_events_are_tagged_json_lines() {
    let mut wire = Vec::new();
    send(&mut wire, &ClientMessage::Subscribe).unwrap();
    send(&mut wire, &DaemonMessage::Subscribed { protocol: 1 }).unwrap();
    let started = DaemonMessage::Event {
        protocol: 1,
        seq: 2,
        event: JobEvent::Started {
            job: 3,
            unit: "taskslash-1-3".into(),
            launch: Launch::Plain,
            log: "/logs/taskslash-1-3.log".into(),
            at: "2026-11-17T00:00:00.000Z".into(),
        },
    };
    send(&mut wire, &started).unwrap();
    let dropped = DaemonMessage::Event {
        protocol: 1,
        seq: 9,
        event: JobEvent::Dropped { count: 4 },
    };
    send(&mut wire, &dropped).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&wire),
        "{\"type\":\"subscribe\"}\n\
         {\"type\":\"subscribed\",\"protocol\":1}\n\
         {\"type\":\"event\",\"protocol\":1,\"seq\":2,\"event\":{\"type\":\"started\",\"job\":3,\"unit\":\"taskslash-1-3\",\"launch\":\"plain\",\"log\":\"/logs/taskslash-1-3.log\",\"at\":\"2026-11-17T00:00:00.000Z\"}}\n\
         {\"type\":\"event\",\"protocol\":1,\"seq\":9,\"event\":{\"type\":\"dropped\",\"count\":4}}\n"
    );
    let mut reader = wire.as_slice();
    assert_eq!(
        receive(&mut reader).unwrap(),
        Some(ClientMessage::Subscribe)
    );
    assert_eq!(
        receive(&mut reader).unwrap(),
        Some(DaemonMessage::Subscribed { protocol: 1 })
    );
    assert_eq!(receive(&mut reader).unwrap(), Some(started));
    assert_eq!(receive(&mut reader).unwrap(), Some(dropped));
}

#[test]
fn a_finished_event_spells_its_unknowns_as_null() {
    let finished = JobEvent::Finished {
        job: 1,
        key: "k".into(),
        exit_code: None,
        wall_ms: 0,
        cached: None,
        moon_hash: None,
        run: None,
        log: None,
        at: "2026-11-17T00:00:00.000Z".into(),
    };
    assert_eq!(
        serde_json::to_string(&finished).unwrap(),
        "{\"type\":\"finished\",\"job\":1,\"key\":\"k\",\"exit_code\":null,\"wall_ms\":0,\"cached\":null,\"moon_hash\":null,\"run\":null,\"log\":null,\"at\":\"2026-11-17T00:00:00.000Z\"}"
    );
}

#[test]
fn an_old_clients_history_parses_and_a_new_one_carries_limit_and_where() {
    let old: ClientMessage = serde_json::from_str("{\"type\":\"history\",\"key\":\"k\"}").unwrap();
    assert_eq!(
        old,
        ClientMessage::History {
            key: "k".into(),
            limit: None,
            within: None,
        }
    );
    let new = ClientMessage::History {
        key: "k".into(),
        limit: Some(3),
        within: Some("wt".into()),
    };
    assert_eq!(
        serde_json::to_string(&new).unwrap(),
        "{\"type\":\"history\",\"key\":\"k\",\"limit\":3,\"where\":\"wt\"}"
    );
    assert_eq!(
        serde_json::to_string(&ClientMessage::Keys).unwrap(),
        "{\"type\":\"keys\"}"
    );
}

#[test]
fn suggestions_are_substring_matches_first_then_the_closest_names() {
    let keys: Vec<String> = [
        "shop/web:test",
        "shop/api:test",
        "shop/web:build",
        "bun test",
        "gate",
        "gates",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        suggest("WEB", &keys),
        ["shop/web:test", "shop/web:build"],
        "case-insensitive substring, in the keys' order"
    );
    assert_eq!(
        suggest("GAET", &keys),
        ["gate", "gates"],
        "else closest by Jaro, any case"
    );
    let near: Vec<String> = ["gate", "late"].into_iter().map(String::from).collect();
    assert_eq!(
        suggest("gat", &near),
        ["gate"],
        "no Jaro once a key contains it"
    );
    assert_eq!(
        suggest("shop", &keys),
        ["shop/web:test", "shop/api:test", "shop/web:build"]
    );
    assert!(suggest("zzzzzz", &keys).is_empty());
    let many: Vec<String> = (0..9).map(|n| format!("job{n}")).collect();
    assert_eq!(suggest("job", &many).len(), 5, "at most five");
}

#[test]
fn where_keeps_a_full_path_or_a_directory_name() {
    assert!(submitted_from("/dev/shop", "shop"));
    assert!(submitted_from("/dev/shop", "/dev/shop"));
    assert!(!submitted_from("/dev/myshop", "shop"));
    assert!(!submitted_from("/dev/shop", "hop"));
    assert!(submitted_from("shop", "shop"));
}
