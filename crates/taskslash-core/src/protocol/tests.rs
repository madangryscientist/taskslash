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
    };
    let history = ClientMessage::History {
        key: "probe".into(),
    };
    let mut wire = Vec::new();
    send(&mut wire, &submit).unwrap();
    send(&mut wire, &start).unwrap();
    send(&mut wire, &history).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&wire),
        "{\"type\":\"submit\",\"name\":\"probe\",\"kind\":\"browser\",\"argv\":[\"sh\",\"-c\",\"exit 3\"],\"cwd\":\"/work\",\"class\":\"flake\",\"submitter\":\"/work\"}\n\
         {\"type\":\"start\",\"job\":3,\"unit\":\"taskslash-1-3\",\"launch\":\"scope\"}\n\
         {\"type\":\"history\",\"key\":\"probe\"}\n"
    );
    let mut reader = wire.as_slice();
    assert_eq!(receive(&mut reader).unwrap(), Some(submit));
    assert_eq!(receive(&mut reader).unwrap(), Some(start));
    assert_eq!(receive(&mut reader).unwrap(), Some(history));
    assert_eq!(receive::<DaemonMessage>(&mut reader).unwrap(), None);
}
