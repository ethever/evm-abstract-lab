use super::*;
use evm_abstract_protocol::{
    DecimalU64, DecompileHashSource, DecompileJobSnapshot, DecompileRequest, DecompileSource,
    JobId, PublicationVersion,
};

fn opened(request: DecompileRequest, id: u64) -> DecompileOpened {
    let bytes = runtime(&request.bytecode).unwrap();
    let source = DecompileSource {
        sha256: decompile_sha256(&bytes),
        config_sha256: DecompileSettings::from_request(&request).sha256(),
        origin: None,
        cache_hit: true,
    };
    DecompileOpened {
        request,
        source,
        snapshot: DecompileJobSnapshot {
            id: JobId(id),
            state: JobState::Completed,
            progress: Default::default(),
            queue_ns: DecimalU64(0),
            processing_ns: DecimalU64(0),
            publication: Some(PublicationVersion(DecimalU64(1))),
            cache_warning: None,
        },
    }
}

fn lookup(opened: &DecompileOpened) -> DecompileOpenRequest {
    DecompileOpenRequest {
        source: DecompileSourceInput::Sha256(DecompileHashSource {
            sha256: opened.source.sha256.clone(),
            config_sha256: Some(opened.source.config_sha256.clone()),
        }),
        settings: None,
    }
}

#[test]
fn source_and_full_settings_are_checked_before_acceptance() {
    let mut request = DecompileRequest::default();
    request.limits.max_work = 123;
    let valid = opened(request, 9);
    let requested = lookup(&valid);
    assert!(validate(&requested, &valid).is_ok());
    let mut changed = valid.clone();
    changed.request.bytecode = "fe".into();
    assert!(validate(&requested, &changed).is_err());
    changed.source.sha256 = decompile_sha256(&[0xfe]);
    assert!(validate(&requested, &changed).is_err());
    let mut changed = valid.clone();
    changed.request.limits.max_work += 1;
    changed.source.config_sha256 = DecompileSettings::from_request(&changed.request).sha256();
    assert!(validate(&requested, &changed).is_err());
}

#[test]
fn stale_openings_never_install_and_cancelled_openings_still_cleanup() {
    let valid = opened(DecompileRequest::default(), 9);
    let mut task = Task::default();
    let Command::Open {
        generation: old, ..
    } = task.open(lookup(&valid))
    else {
        unreachable!()
    };
    task.request_cancel();
    task.retire();
    let mut running = valid.clone();
    running.snapshot.state = JobState::Running;
    running.snapshot.publication = None;
    running.source.cache_hit = false;
    let reply = Ok(DecompileOpenReply {
        result: Ok(running),
    });
    assert!(matches!(
        task.stale_open_cleanup(old, &reply),
        Some(Command::Cancel { id: JobId(9), .. })
    ));
    assert!(task.receive_open(old, reply).is_none());
    assert!(task.snapshot.is_none());
    assert!(
        task.stale_open_cleanup(old, &Ok(DecompileOpenReply { result: Ok(valid) }))
            .is_none()
    );
}

#[test]
fn cancelling_while_resolving_waits_for_the_actual_job_identity() {
    let mut valid = opened(DecompileRequest::default(), 19);
    valid.snapshot.state = JobState::Queued;
    valid.snapshot.publication = None;
    valid.source.cache_hit = false;
    let mut task = Task::default();
    let Command::Open { generation, .. } = task.open(lookup(&valid)) else {
        unreachable!()
    };
    task.request_cancel();
    assert!(task.next(0.0).is_none());
    assert!(
        task.receive_open(generation, Ok(DecompileOpenReply { result: Ok(valid) }))
            .is_some()
    );
    assert!(matches!(
        task.take_cancel(),
        Some(Command::Cancel { id: JobId(19), .. })
    ));
    assert!(task.take_cancel().is_none());
}

#[test]
fn failed_retired_openings_release_their_cancellation_record() {
    let valid = opened(DecompileRequest::default(), 20);
    let mut task = Task::default();
    let Command::Open { generation, .. } = task.open(lookup(&valid)) else {
        unreachable!()
    };
    task.request_cancel();
    task.retire();
    assert!(task.cancelled_openings.contains_key(&generation));
    assert!(
        task.stale_open_cleanup(generation, &Err(TransportError::Http { status: 404 }))
            .is_none()
    );
    assert!(!task.cancelled_openings.contains_key(&generation));
}

#[test]
fn retired_cancellation_cannot_cancel_a_current_job_from_a_wrong_source_or_config_receipt() {
    for wrong_config in [false, true] {
        let mut a = opened(DecompileRequest::default(), 30);
        a.source.cache_hit = false;
        a.snapshot.state = JobState::Queued;
        a.snapshot.publication = None;
        let mut task = Task::default();
        let Command::Open {
            generation: old, ..
        } = task.open(lookup(&a))
        else {
            unreachable!()
        };
        task.request_cancel();
        let mut request = DecompileRequest::default();
        if wrong_config {
            request.limits.max_work = 123;
        } else {
            request.bytecode = "fe".into();
        }
        let mut b = opened(request, 31);
        b.source.cache_hit = false;
        b.snapshot.state = JobState::Running;
        b.snapshot.publication = None;
        let Command::Open { generation, .. } = task.open(lookup(&b)) else {
            unreachable!()
        };
        assert!(
            task.receive_open(
                generation,
                Ok(DecompileOpenReply {
                    result: Ok(b.clone())
                })
            )
            .is_some()
        );
        assert!(
            task.stale_open_cleanup(old, &Ok(DecompileOpenReply { result: Ok(b) }))
                .is_none()
        );
        assert!(!task.cancelled_openings.contains_key(&old));
        assert_eq!(task.snapshot.as_ref().unwrap().id, JobId(31));
        assert!(task.error.is_none());
    }
}
