use super::*;

async fn until(mut ready: impl FnMut() -> bool) -> Result<(), Error> {
    for _ in 0..500 {
        if ready() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    Err(Error::new(
        "Error",
        "Owned admission barrier was not reached",
    ))
}

#[derive(Clone, Default)]
struct SharedEvents(Rc<RefCell<Vec<Value>>>);

impl Observer for SharedEvents {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        self.0.borrow_mut().push(event);
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn actual_four_requests_share_fifo_two_slot_admission_across_both_chat_apis()
-> Result<(), Error> {
    let admission = Admission::default();
    let config = config(r#"{"fleet_profile":"fifo","concurrency":2}"#)?;
    let trace = Rc::new(Trace::default());
    let gates = (0..4).map(|_| Rc::new(Gate::default())).collect::<Vec<_>>();
    let mut transports = gates
        .iter()
        .map(|gate| Recorded::shared(&trace, vec![Packet::gated(gate)]));
    let mut a = transports
        .next()
        .ok_or_else(|| Error::new("Error", "Missing first transport"))?;
    let mut b = transports
        .next()
        .ok_or_else(|| Error::new("Error", "Missing second transport"))?;
    let mut c = transports
        .next()
        .ok_or_else(|| Error::new("Error", "Missing third transport"))?;
    let mut d = transports
        .next()
        .ok_or_else(|| Error::new("Error", "Missing fourth transport"))?;
    let events = SharedEvents::default();
    let mut oa = events.clone();
    let mut ob = events.clone();
    let mut oc = events.clone();
    let mut od = events.clone();
    let credentials = Credentials::default();
    let mut ca = chat(&config, &admission)?;
    let mut cc = chat(&config, &admission)?;
    let qa = request("a");
    let qb = request("b");
    let qc = request("c");
    let qd = request("d");
    let (ra, rb, rc, rd, control) = tokio::join!(
        ca.complete(&qa, &mut a, &mut oa, &credentials, CallOptions::default()),
        builtin::chat_completion(
            &config,
            &qb,
            &admission,
            &mut b,
            &mut ob,
            &credentials,
            builtin::CompletionOptions::default()
        ),
        cc.complete(&qc, &mut c, &mut oc, &credentials, CallOptions::default()),
        builtin::chat_completion(
            &config,
            &qd,
            &admission,
            &mut d,
            &mut od,
            &credentials,
            builtin::CompletionOptions::default()
        ),
        async {
            until(|| trace.sent.borrow().len() == 2).await?;
            assert_eq!(trace.order(), ["a", "b"]);
            assert_eq!(trace.active.get(), 2);
            assert_eq!(admission.depth(&Text::from("fifo"))?, 2);
            let depths = events
                .0
                .borrow()
                .iter()
                .filter(|event| event.get("queued") == Some(&Value::Boolean(true)))
                .filter_map(|event| event.get("depth").and_then(Value::as_number))
                .collect::<Vec<_>>();
            assert_eq!(depths, [1.0, 2.0]);
            for (index, gate) in gates.iter().enumerate() {
                gate.open();
                if index < 2 {
                    until(|| trace.sent.borrow().len() == index + 3).await?;
                }
            }
            Ok::<_, Error>(())
        }
    );
    control?;
    for result in [
        ra.map(|_| ()),
        rb.map(|_| ()),
        rc.map(|_| ()),
        rd.map(|_| ()),
    ] {
        result.map_err(|failure| *failure.error)?;
    }
    assert_eq!(trace.order(), ["a", "b", "c", "d"]);
    assert_eq!(trace.peak.get(), 2);
    assert_eq!(trace.active.get(), 0);
    assert_eq!(trace.closed.get(), 4);
    assert_eq!(admission.depth(&Text::from("fifo"))?, 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn profiles_are_separate_and_unprofiled_requests_are_not_worker_limited() -> Result<(), Error>
{
    for profiled in [false, true] {
        let admission = Admission::default();
        let config_a = config(if profiled {
            r#"{"fleet_profile":"a","concurrency":1}"#
        } else {
            r#"{"concurrency":1}"#
        })?;
        let config_b = config(if profiled {
            r#"{"fleet_profile":"b","concurrency":1}"#
        } else {
            r#"{"concurrency":1}"#
        })?;
        let trace = Rc::new(Trace::default());
        let gate = Rc::new(Gate::default());
        let mut a = Recorded::shared(&trace, vec![Packet::gated(&gate)]);
        let mut b = Recorded::shared(&trace, vec![Packet::gated(&gate)]);
        let mut ca = chat(&config_a, &admission)?;
        let mut cb = chat(&config_b, &admission)?;
        let qa = request("a");
        let qb = request("b");
        let (a, b, control) = tokio::join!(
            complete(&mut ca, &mut a, &qa),
            complete(&mut cb, &mut b, &qb),
            async {
                until(|| trace.active.get() == 2).await?;
                gate.open();
                Ok::<_, Error>(())
            }
        );
        control?;
        a.map_err(|failure| *failure.error)?;
        b.map_err(|failure| *failure.error)?;
        assert_eq!(trace.peak.get(), 2);
        assert_eq!(trace.active.get(), 0);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn declared_capacity_increase_and_shrink_account_for_unpolled_grants() -> Result<(), Error> {
    let admission = Admission::default();
    let one = config(r#"{"fleet_profile":"reconfigure","concurrency":1}"#)?;
    let three = config(r#"{"fleet_profile":"reconfigure","concurrency":3}"#)?;
    let trace = Rc::new(Trace::default());
    let ga = Rc::new(Gate::default());
    let gb = Rc::new(Gate::default());
    let gc = Rc::new(Gate::default());
    let gd = Rc::new(Gate::default());
    let mut a = Recorded::shared(&trace, vec![Packet::gated(&ga)]);
    let mut b = Recorded::shared(&trace, vec![Packet::gated(&gb)]);
    let mut c = Recorded::shared(&trace, vec![Packet::gated(&gc)]);
    let mut d = Recorded::shared(&trace, vec![Packet::gated(&gd)]);
    let mut ca = chat(&one, &admission)?;
    let mut cb = chat(&one, &admission)?;
    let mut cc = chat(&three, &admission)?;
    let mut cd = chat(&one, &admission)?;
    let qa = request("a");
    let qb = request("b");
    let qc = request("c");
    let qd = request("d");
    let (a, b, c, d, control) = tokio::join!(
        complete(&mut ca, &mut a, &qa),
        complete(&mut cb, &mut b, &qb),
        complete(&mut cc, &mut c, &qc),
        complete(&mut cd, &mut d, &qd),
        async {
            until(|| trace.sent.borrow().len() == 3).await?;
            assert_eq!(trace.active.get(), 3);
            assert_eq!(admission.depth(&Text::from("reconfigure"))?, 3);
            ga.open();
            until(|| trace.active.get() == 2).await?;
            assert_eq!(trace.sent.borrow().len(), 3);
            assert_eq!(admission.depth(&Text::from("reconfigure"))?, 2);
            gb.open();
            until(|| trace.active.get() == 1).await?;
            assert_eq!(trace.sent.borrow().len(), 3);
            gc.open();
            until(|| trace.sent.borrow().len() == 4).await?;
            assert_eq!(trace.order().last().map(String::as_str), Some("d"));
            assert_eq!(trace.active.get(), 1);
            gd.open();
            Ok::<_, Error>(())
        }
    );
    control?;
    for result in [a, b, c, d] {
        result.map_err(|failure| *failure.error)?;
    }
    assert_eq!(trace.peak.get(), 3);
    assert_eq!(trace.active.get(), 0);
    assert_eq!(admission.depth(&Text::from("reconfigure"))?, 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn holder_bound_release_drop_and_reconfiguration_cannot_free_another_holder()
-> Result<(), Error> {
    let admission = Admission::default();
    let profile = Text::from("holders");
    assert_eq!(admission.depth(&profile)?, 0);
    admission.configure(&profile, 2)?;
    let mut first = admission.acquire(&profile, None, None)?.wait().await?;
    let second = admission.acquire(&profile, None, None)?.wait().await?;
    let third = admission.acquire(&profile, None, None)?;
    assert!(!third.granted()?);
    assert_eq!(admission.depth(&profile)?, 1);
    assert!(first.release()?);
    assert!(third.granted()?);
    assert!(!first.release()?);
    let fourth = admission.acquire(&profile, None, None)?;
    assert!(!fourth.granted()?);
    admission.configure(&profile, 1)?;
    assert_eq!(admission.depth(&profile)?, 2);
    drop(first);
    assert!(!fourth.granted()?);
    drop(second);
    assert!(!fourth.granted()?);
    drop(third);
    assert!(fourth.granted()?);
    let mut fourth = fourth.wait().await?;
    assert!(fourth.release()?);
    assert!(!fourth.release()?);
    assert_eq!(admission.depth(&profile)?, 0);
    assert!(admission.configure(&profile, 0).is_err());
    assert!(
        admission
            .configure(&profile, crate::MAX_SAFE_INTEGER + 1)
            .is_err()
    );
    assert!(admission.acquire(&Text::default(), None, None).is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn default_capacity_fifo_skips_cancelled_waiters_without_losing_the_first_reason()
-> Result<(), Error> {
    let admission = Admission::default();
    let profile = Text::from("default");
    let mut holder = admission.acquire(&profile, None, None)?.wait().await?;
    let parent = CancellationSource::default();
    let child = parent.token().child_source()?;
    let token = child.token();
    let cancelled = admission.acquire(&profile, None, Some(&token))?;
    let next = admission.acquire(&profile, None, None)?;
    let last = admission.acquire(&profile, None, None)?;
    assert_eq!(admission.depth(&profile)?, 3);
    child.cancel(Reason::TaskTimeout(450))?;
    parent.cancel(Reason::Cancelled)?;
    assert!(holder.release()?);
    assert!(next.granted()?);
    assert!(!last.granted()?);
    let error = cancelled
        .wait()
        .await
        .err()
        .ok_or_else(|| Error::new("Error", "Cancelled waiter succeeded"))?;
    assert_eq!(error.code, Some("ROSTER_TASK_TIMEOUT"));
    assert_eq!(token.reason()?, Some(Reason::TaskTimeout(450)));
    drop(next);
    assert!(last.granted()?);
    drop(last);
    assert!(admission.acquire(&profile, None, Some(&token)).is_err());
    assert_eq!(admission.depth(&profile)?, 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn dropping_a_real_queued_or_just_granted_future_releases_only_its_reservation()
-> Result<(), Error> {
    for granted in [false, true] {
        let admission = Admission::default();
        let profile = Text::from("future-drop");
        let mut holder = admission.acquire(&profile, None, None)?.wait().await?;
        let mut chat = chat(&config(r#"{"fleet_profile":"future-drop"}"#)?, &admission)?;
        let mut transport = Recorded::new(vec![packet("stop", "must-not-send", None)]);
        let request = request("dropped");
        let mut future = Box::pin(complete(&mut chat, &mut transport, &request));
        tokio::select! {
            biased;
            _ = &mut future => return Err(Error::new("Error", "Queued future unexpectedly completed")),
            () = tokio::time::sleep(Duration::from_millis(1)) => {}
        }
        assert_eq!(admission.depth(&profile)?, 1);
        let next = admission.acquire(&profile, None, None)?;
        if granted {
            holder.release()?;
        }
        drop(future);
        assert_eq!(chat.request_count(), 0);
        assert_eq!(chat.last_attempts(), 1);
        assert!(transport.trace.sent.borrow().is_empty());
        if granted {
            assert!(next.granted()?);
        } else {
            assert!(!next.granted()?);
            holder.release()?;
            assert!(next.granted()?);
        }
        let mut next = next.wait().await?;
        next.release()?;
        assert_eq!(admission.depth(&profile)?, 0);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancelling_a_queued_or_just_granted_real_request_cannot_send_or_reorder()
-> Result<(), Error> {
    for granted in [false, true] {
        let admission = Admission::default();
        let profile = Text::from("cancelled");
        let mut holder = admission.acquire(&profile, None, None)?.wait().await?;
        let source = CancellationSource::default();
        let token = source.token();
        let mut chat = chat(&config(r#"{"fleet_profile":"cancelled"}"#)?, &admission)?;
        let mut transport = Recorded::new(vec![]);
        let mut observer = NoObserver;
        let credentials = Credentials::default();
        let request = request("cancelled");
        let mut future = Box::pin(chat.complete(
            &request,
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions {
                signal: Some(&token),
                ..CallOptions::default()
            },
        ));
        tokio::select! {
            biased;
            _ = &mut future => return Err(Error::new("Error", "Queued future unexpectedly completed")),
            () = tokio::time::sleep(Duration::from_millis(1)) => {}
        }
        let next = admission.acquire(&profile, None, None)?;
        let last = admission.acquire(&profile, None, None)?;
        if granted {
            holder.release()?;
        }
        source.cancel(Reason::TaskTimeout(450))?;
        source.cancel(Reason::Cancelled)?;
        let error = failed(future.await)?;
        assert_eq!(error.error.code, Some("ROSTER_TASK_TIMEOUT"));
        assert_eq!(chat.request_count(), 0);
        assert!(transport.trace.sent.borrow().is_empty());
        if !granted {
            assert!(!next.granted()?);
            holder.release()?;
        }
        assert!(next.granted()?);
        assert!(!last.granted()?);
        drop(next);
        assert!(last.granted()?);
        drop(last);
        assert_eq!(admission.depth(&profile)?, 0);
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn dropping_an_inflight_real_transport_future_drops_body_and_slot_synchronously()
-> Result<(), Error> {
    let admission = Admission::default();
    let profile = Text::from("inflight-drop");
    let mut chat = chat(&config(r#"{"fleet_profile":"inflight-drop"}"#)?, &admission)?;
    let gate = Rc::new(Gate::default());
    let mut transport = Recorded::new(vec![Packet::gated(&gate)]);
    let trace = transport.trace.clone();
    let request = request("inflight");
    let mut future = Box::pin(complete(&mut chat, &mut transport, &request));
    tokio::select! {
        biased;
        _ = &mut future => return Err(Error::new("Error", "Gated request unexpectedly completed")),
        () = tokio::time::sleep(Duration::from_millis(1)) => {}
    }
    assert_eq!(trace.active.get(), 1);
    let pending = admission.acquire(&profile, None, None)?;
    assert!(!pending.granted()?);
    drop(future);
    assert_eq!(trace.active.get(), 0);
    assert_eq!(trace.closed.get(), 1);
    assert!(pending.granted()?);
    assert_eq!(chat.request_count(), 1);
    assert!(chat.last_response().is_none());
    drop(pending);
    assert_eq!(admission.depth(&profile)?, 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn already_cancelled_wrapper_does_not_change_capacity_or_touch_transport() -> Result<(), Error>
{
    let admission = Admission::default();
    let profile = Text::from("already");
    let holder = admission.acquire(&profile, None, None)?.wait().await?;
    let source = CancellationSource::default();
    source.cancel(Reason::Cancelled)?;
    let token = source.token();
    let mut chat = chat(
        &config(r#"{"fleet_profile":"already","concurrency":3}"#)?,
        &admission,
    )?;
    let mut transport = Recorded::new(vec![]);
    let error = failed(
        chat.complete(
            &request("Ask"),
            &mut transport,
            &mut NoObserver,
            &Credentials::default(),
            CallOptions {
                parent: Some(&token),
                ..CallOptions::default()
            },
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_CANCELLED"));
    assert_eq!(chat.request_count(), 0);
    assert_eq!(chat.last_attempts(), 0);
    let pending = admission.acquire(&profile, None, None)?;
    assert!(!pending.granted()?);
    drop(holder);
    assert!(pending.granted()?);
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn queue_heartbeats_are_thirty_second_redacted_depth_observations_not_http_sends()
-> Result<(), Error> {
    let admission = Admission::default();
    let profile = Text::from("heartbeat");
    let mut holder = admission.acquire(&profile, None, None)?.wait().await?;
    let mut chat = chat(
        &config(
            r#"{"base_url":"http://fixture-selected-key.invalid:8000/private/path","api_key_env":"SEAT_KEY","fleet_profile":"heartbeat","request_timeout_ms":50}"#,
        )?,
        &admission,
    )?;
    let mut transport = Recorded::new(vec![packet("stop", "ok", None)]);
    let trace = transport.trace.clone();
    let events = SharedEvents::default();
    let mut observer = events.clone();
    let credentials = Credentials {
        environment: None,
        vault_entries: Some(parse(r#"{"SEAT_KEY":"fixture-selected-key"}"#)?),
    };
    let request = request("Ask");
    let (response, control) = tokio::join!(
        chat.complete(
            &request,
            &mut transport,
            &mut observer,
            &credentials,
            CallOptions::default()
        ),
        async {
            until(|| !events.0.borrow().is_empty()).await?;
            assert!(trace.sent.borrow().is_empty());
            tokio::time::advance(Duration::from_millis(29_998)).await;
            tokio::task::yield_now().await;
            assert_eq!(events.0.borrow().len(), 1);
            tokio::time::advance(Duration::from_millis(1)).await;
            until(|| events.0.borrow().len() >= 2).await?;
            tokio::time::advance(Duration::from_secs(30)).await;
            until(|| events.0.borrow().len() >= 3).await?;
            let events = events.0.borrow();
            let queued = events
                .iter()
                .filter(|event| event.get("queued") == Some(&Value::Boolean(true)))
                .collect::<Vec<_>>();
            assert_eq!(queued.len(), 3);
            for (event, elapsed) in queued.iter().zip([0.0, 30.0, 60.0]) {
                assert_eq!(event.get("elapsedSeconds"), Some(&Value::Number(elapsed)));
                assert_eq!(event.get("depth"), Some(&Value::Number(1.0)));
                assert_eq!(
                    event.get("host"),
                    Some(&Value::string("[redacted].invalid:8000"))
                );
                assert!(!event.json().contains("private/path"));
                assert!(!event.json().contains("fixture-selected-key"));
            }
            drop(events);
            holder.release()?;
            Ok::<_, Error>(())
        }
    );
    control?;
    response.map_err(|failure| *failure.error)?;
    assert_eq!(chat.request_count(), 1);
    assert_eq!(trace.active.get(), 0);
    assert_eq!(admission.depth(&profile)?, 0);
    Ok(())
}

struct GrantThenFail<'a> {
    admission: &'a Admission,
    profile: Text,
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn invalid_retry_hints_cannot_reach_queued_observers_or_timeout_errors() -> Result<(), Error>
{
    let admission = Admission::default();
    let profile = Text::from("invalid-retry");
    let holder = admission.acquire(&profile, None, None)?.wait().await?;
    let mut chat = chat(
        &config(r#"{"fleet_profile":"invalid-retry","request_timeout_ms":10}"#)?,
        &admission,
    )?;
    let mut transport = Recorded::new(vec![]);
    let mut observer = Events {
        fail_type: Some("waiting"),
        hang: true,
        ..Events::default()
    };
    let error = failed(
        chat.complete(
            &request("Ask"),
            &mut transport,
            &mut observer,
            &Credentials::default(),
            CallOptions {
                retry_command: Some("unapproved command\nPRIVATE_HINT"),
                ..CallOptions::default()
            },
        )
        .await,
    )?;
    assert_eq!(error.error.name, "TypeError");
    assert!(observer.values.is_empty());
    assert!(!error.error.message.contains("PRIVATE_HINT"));
    assert_eq!(chat.request_count(), 0);
    assert_eq!(admission.depth(&profile)?, 0);
    let next = admission.acquire(&profile, None, None)?;
    assert!(!next.granted()?);
    drop(holder);
    assert!(next.granted()?);
    Ok(())
}

impl Observer for GrantThenFail<'_> {
    async fn emit(&mut self, event: Value) -> Result<(), Error> {
        if event.get("queued") == Some(&Value::Boolean(true)) {
            self.admission.configure(&self.profile, 2)?;
            return Err(
                Error::new("Error", "Owned queued observer failure").coded("ROSTER_RUN_LOG")
            );
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn queue_observer_failure_or_hang_releases_pending_and_newly_granted_reservations()
-> Result<(), Error> {
    let admission = Admission::default();
    let profile = Text::from("queue-observer");
    let holder = admission.acquire(&profile, None, None)?.wait().await?;
    let mut chat = chat(
        &config(r#"{"fleet_profile":"queue-observer","request_timeout_ms":10}"#)?,
        &admission,
    )?;
    let mut transport = Recorded::new(vec![]);
    let error = failed(
        chat.complete(
            &request("Ask"),
            &mut transport,
            &mut GrantThenFail {
                admission: &admission,
                profile: profile.clone(),
            },
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_RUN_LOG"));
    let spare = admission.acquire(&profile, None, None)?.wait().await?;
    let blocked = admission.acquire(&profile, None, None)?;
    assert!(!blocked.granted()?);
    drop(blocked);
    drop(spare);
    admission.configure(&profile, 1)?;
    let mut observer = Events {
        fail_type: Some("waiting"),
        hang: true,
        ..Events::default()
    };
    let error = failed(
        chat.complete(
            &request("Ask again"),
            &mut transport,
            &mut observer,
            &Credentials::default(),
            CallOptions::default(),
        )
        .await,
    )?;
    assert_eq!(error.error.code, Some("ROSTER_LLM_TIMEOUT"));
    assert_eq!(chat.request_count(), 0);
    assert_eq!(admission.depth(&profile)?, 0);
    drop(holder);
    assert!(admission.acquire(&profile, None, None)?.granted()?);
    Ok(())
}
