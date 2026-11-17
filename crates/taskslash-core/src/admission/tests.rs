//! Each branch of the rule against a fixed machine: 10 threads, 64 GiB, idle, calm, roomy disk.

use super::*;
use crate::procfs::Pressure;
use crate::sampler::{Cpu, Disk, Io, Memory, Mount};

/// A calm machine: 10 threads, 64 GiB, idle, no pressure, a roomy disk.
pub(crate) fn machine() -> MachineSnapshot {
    MachineSnapshot {
        sampled_at: "2026-11-16T00:00:00.000Z".into(),
        cpu: Cpu {
            threads: 10,
            threads_online: 12,
            busy: 0.0,
            pressure: Pressure::default(),
        },
        memory: Memory {
            total_mib: 65536,
            available_mib: 65536,
            swap_total_mib: 0,
            swap_free_mib: 0,
            pressure: Pressure::default(),
        },
        io: Io {
            pressure: Pressure::default(),
            disks: vec![Disk {
                name: "nvme0n1".into(),
                busy: 0.0,
                write_mib_s: 0.0,
            }],
        },
        mounts: vec![Mount {
            path: "/home".into(),
            total_mib: 1 << 20,
            free_mib: 900_000,
            used: 0.14,
        }],
        load: [0.0; 3],
    }
}

fn demand(cpu: f64) -> Demand {
    Demand {
        estimate: Use {
            cpu,
            mem_mib: 100.0,
            write_mib_s: 0.0,
        },
        never_seen: false,
    }
}

fn job(id: u64, cpu: f64) -> Running {
    Running {
        job: id,
        key: format!("j{id}"),
        estimate: demand(cpu).estimate,
        observed: None,
        never_seen: false,
    }
}

fn decide(m: &MachineSnapshot, running: &[Running], candidate: &Demand) -> Decision {
    check(m, &Use::default(), running, candidate, &Limits::default())
}

fn gate_of(decision: Decision) -> Gate {
    match decision {
        Decision::Wait(Reason::Gate { gate, .. }) => gate,
        other => panic!("not a gate: {other:?}"),
    }
}

fn resource_of(decision: &Decision) -> (Resource, f64, f64, Vec<u64>) {
    match decision {
        Decision::Wait(Reason::Resource {
            resource,
            needs,
            free,
            held_by,
        }) => (
            *resource,
            *needs,
            *free,
            held_by.iter().map(|h| h.job).collect(),
        ),
        other => panic!("not a resource: {other:?}"),
    }
}

#[test]
fn each_gate_trips_on_its_own() {
    let mut m = machine();
    m.cpu.pressure.some_avg10 = 41.0;
    assert_eq!(gate_of(decide(&m, &[], &demand(0.1))), Gate::CpuPressure);
    let mut m = machine();
    m.memory.pressure.some_avg10 = 5.5;
    assert_eq!(gate_of(decide(&m, &[], &demand(0.1))), Gate::MemoryPressure);
    let mut m = machine();
    m.io.pressure.full_avg10 = 6.0;
    assert_eq!(gate_of(decide(&m, &[], &demand(0.1))), Gate::IoFullPressure);
    let mut m = machine();
    m.io.pressure.some_avg10 = 26.0;
    assert_eq!(gate_of(decide(&m, &[], &demand(0.1))), Gate::IoPressure);
    let mut m = machine();
    m.memory.available_mib = 6000;
    assert_eq!(
        gate_of(decide(&m, &[], &demand(0.1))),
        Gate::MemoryAvailable
    );
    let mut m = machine();
    m.mounts[0].used = 0.91;
    let reason = decide(&m, &[], &demand(0.1));
    assert_eq!(
        reason,
        Decision::Wait(Reason::Gate {
            gate: Gate::Mount,
            value: 91.0,
            limit: 90.0,
            mount: Some("/home".into())
        })
    );
    // At the limit is not over it.
    let mut m = machine();
    m.cpu.pressure.some_avg10 = 40.0;
    m.mounts[0].used = 0.9;
    assert_eq!(decide(&m, &[], &demand(0.1)), Decision::Start);
}

#[test]
fn a_gate_comes_before_the_never_seen_rule_and_that_before_a_resource() {
    let mut m = machine();
    m.cpu.pressure.some_avg10 = 50.0;
    let mut other = job(1, 9.0);
    other.never_seen = true;
    let mut candidate = demand(4.0);
    candidate.never_seen = true;
    let reason = decide(&m, std::slice::from_ref(&other), &candidate);
    assert_eq!(gate_of(reason), Gate::CpuPressure);
    m.cpu.pressure.some_avg10 = 0.0;
    assert_eq!(
        decide(&m, std::slice::from_ref(&other), &candidate),
        Decision::Wait(Reason::NeverSeen {
            job: 1,
            key: "j1".into()
        })
    );
    candidate.never_seen = false;
    let (resource, ..) = resource_of(&decide(&m, &[other], &candidate));
    assert_eq!(resource, Resource::Cpu);
}

#[test]
fn memory_blocks_on_its_own() {
    // Budget 0.8 * 65536 = 52428.8 MiB.
    let m = machine();
    let mut big = job(1, 0.1);
    big.estimate.mem_mib = 50_000.0;
    let mut candidate = demand(0.1);
    candidate.estimate.mem_mib = 3_000.0;
    let (resource, needs, free, held) = resource_of(&decide(&m, &[big], &candidate));
    assert_eq!((resource, needs, held), (Resource::Memory, 3000.0, vec![1]));
    assert!((free - 2428.8).abs() < 1e-6, "{free}");
}

#[test]
fn writes_block_on_their_own() {
    let m = machine();
    let mut writer = job(1, 0.1);
    writer.estimate.write_mib_s = 100.0;
    let mut candidate = demand(0.1);
    candidate.estimate.write_mib_s = 60.0;
    let (resource, needs, free, _) = resource_of(&decide(&m, &[writer], &candidate));
    assert_eq!((resource, needs, free), (Resource::Write, 60.0, 50.0));
}

#[test]
fn a_job_too_big_to_ever_fit_starts_alone() {
    let m = machine();
    assert_eq!(decide(&m, &[], &demand(12.0)), Decision::Start);
    let (resource, _, free, _) = resource_of(&decide(&m, &[job(1, 0.5)], &demand(12.0)));
    assert_eq!((resource, free), (Resource::Cpu, 8.0));
    // Alone but under a gate: it waits like any other.
    let mut m = machine();
    m.io.pressure.some_avg10 = 30.0;
    assert_eq!(gate_of(decide(&m, &[], &demand(12.0))), Gate::IoPressure);
}

#[test]
fn held_by_is_largest_share_first() {
    let m = machine();
    let running = [job(1, 1.0), job(2, 4.0), job(3, 2.0), job(4, 2.0)];
    let (.., held) = resource_of(&decide(&m, &running, &demand(2.0)));
    assert_eq!(held, [2, 3, 4, 1]);
}

#[test]
fn a_job_observed_above_its_estimate_counts_at_the_observed_value() {
    let m = machine();
    let mut grown = job(1, 2.0);
    grown.observed = Some(Use {
        cpu: 6.0,
        mem_mib: 10.0,
        write_mib_s: 0.0,
    });
    let decision = decide(&m, std::slice::from_ref(&grown), &demand(3.0));
    let (_, _, free, _) = resource_of(&decision);
    assert!((free - 2.5).abs() < 1e-9, "{free}");
    // Observed below the estimate: the estimate holds its place.
    grown.observed = Some(Use::default());
    assert_eq!(decide(&m, &[grown], &demand(3.0)), Decision::Start);
}

#[test]
fn unmanaged_use_is_added_to_the_committed_sum() {
    let m = machine();
    let unmanaged = Use {
        cpu: 5.0,
        ..Use::default()
    };
    let limits = Limits::default();
    assert_eq!(
        check(&m, &Use::default(), &[], &demand(4.0), &limits),
        Decision::Start
    );
    let decision = check(&m, &unmanaged, &[], &demand(4.0), &limits);
    let (resource, _, free, held) = resource_of(&decision);
    assert_eq!((resource, held), (Resource::Cpu, vec![]));
    assert!((free - 3.5).abs() < 1e-9, "{free}");
}

#[test]
fn unmanaged_is_the_machine_minus_the_managed_floored_at_0() {
    let mut m = machine();
    m.cpu.busy = 0.3;
    m.memory.available_mib = 60_536;
    m.io.disks[0].write_mib_s = 20.0;
    let managed = Use {
        cpu: 1.0,
        mem_mib: 8000.0,
        write_mib_s: 5.0,
    };
    let u = unmanaged(&m, &managed);
    assert!((u.cpu - 2.0).abs() < 1e-9, "{u:?}");
    assert_eq!((u.mem_mib, u.write_mib_s), (0.0, 15.0));
}

#[test]
fn reasons_read_as_the_spikes_sentences() {
    let reason = Reason::Resource {
        resource: Resource::Cpu,
        needs: 4.8,
        free: 1.8,
        held_by: vec![
            Held {
                job: 3,
                key: "cli-db:test".into(),
                amount: 3.0,
            },
            Held {
                job: 5,
                key: "visual".into(),
                amount: 2.0,
            },
        ],
    };
    assert_eq!(
        waiting_line(&Waiting {
            job: None,
            reason: &reason,
            waited_s: 45,
            position: 2,
            never_run: None,
        }),
        "taskslash: waiting 45s (2nd in line): cpu needs 4.8, free 1.8; held by cli-db:test (job 3), visual (job 5)"
    );
    let gate = Reason::Gate {
        gate: Gate::CpuPressure,
        value: 52.0,
        limit: 40.0,
        mount: None,
    };
    assert_eq!(
        waiting_line(&Waiting {
            job: None,
            reason: &gate,
            waited_s: 30,
            position: 1,
            never_run: None,
        }),
        "taskslash: waiting 30s (1st in line): cpu pressure 52% over the 40% gate"
    );
    let ordinals: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 111].map(ordinal).into();
    assert_eq!(
        ordinals,
        [
            "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "111th"
        ]
    );
}

fn holders(n: u64) -> Vec<Held> {
    (1..=n)
        .map(|job| Held {
            job,
            key: format!("k{job}"),
            amount: (20 - job) as f64,
        })
        .collect()
}

#[test]
fn a_waiting_line_starts_with_the_jobs_number() {
    let reason = Reason::Resource {
        resource: Resource::Cpu,
        needs: 4.0,
        free: 1.5,
        held_by: holders(2),
    };
    assert_eq!(
        waiting_line(&Waiting {
            job: Some(18),
            reason: &reason,
            waited_s: 0,
            position: 1,
            never_run: None,
        }),
        "taskslash: job 18 waiting 0s (1st in line): cpu needs 4.0, free 1.5; held by k1 (job 1), k2 (job 2)"
    );
}

#[test]
fn a_reason_names_the_three_biggest_holders_and_counts_the_rest() {
    let reason = Reason::Resource {
        resource: Resource::Memory,
        needs: 4096.0,
        free: 100.0,
        held_by: holders(11),
    };
    assert_eq!(
        reason.to_string(),
        "memory needs 4096 MiB, free 100 MiB; held by k1 (job 1), k2 (job 2), k3 (job 3) and 8 more"
    );
    let three = Reason::Resource {
        resource: Resource::Cpu,
        needs: 4.0,
        free: 0.0,
        held_by: holders(3),
    };
    assert!(three.to_string().ends_with("k3 (job 3)"), "{three}");
}

#[test]
fn a_never_run_keys_first_line_says_what_it_was_assumed_to_take() {
    let gate = Reason::Gate {
        gate: Gate::CpuPressure,
        value: 52.0,
        limit: 40.0,
        mount: None,
    };
    assert_eq!(
        waiting_line(&Waiting {
            job: Some(4),
            reason: &gate,
            waited_s: 0,
            position: 1,
            never_run: Some(2.0),
        }),
        "taskslash: job 4 waiting 0s (1st in line): cpu pressure 52% over the 40% gate \
         (never run before: assumed 2.0 threads; --cpu 0.1 says otherwise)"
    );
}
