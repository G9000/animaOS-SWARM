#[tokio::test]
async fn supervision_proposal_requires_approval_and_budget_is_idempotent() {
    let (service, agent, path) = setup();
    let job = service
        .create_with_controls(&agent, "proposal", "prompt", "proposal", 1, true)
        .await
        .unwrap();
    assert_eq!(job.status, AgentJobStatus::AwaitingApproval);
    let snapshot = load_control_plane_snapshot(&ControlPlaneStoreConfig::Json(path.clone()))
        .await
        .unwrap()
        .unwrap();
    let mut restored = DaemonState::new();
    restored.set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    restored.restore_control_plane_snapshot(snapshot).unwrap();
    let state = Arc::new(RwLock::new(restored));
    let runs = AgentRunCoordinator::new(state.clone(), Arc::new(Semaphore::new(4)));
    let service = JobService::new(state, runs);
    service.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(service.list(&agent).await.unwrap()[0].attempt, 0);
    assert!(service
        .create_with_controls(&agent, "proposal", "prompt", "proposal", 2, true)
        .await
        .is_err());
    assert!(service.approve(&agent, &job.id, 99).await.is_err());
    service
        .approve(&agent, &job.id, job.revision)
        .await
        .unwrap();
    let done = wait_completed(&service, &job.id).await;
    assert_eq!(done.attempts.len(), 1);
    assert!(done.approved_at_ms.is_some());
    let reviewed = service
        .review_output(
            &agent,
            &job.id,
            done.revision,
            JobReviewDecision::ChangesRequested,
            "Revise the conclusion",
        )
        .await
        .unwrap();
    assert!(service
        .retry(&agent, &job.id, reviewed.revision, false)
        .await
        .is_err());
    service.shutdown().await;
    let _ = std::fs::remove_file(path);
}

async fn wait_completed(service: &JobService, id: &str) -> AgentJobRecord {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let job = service.state.read().await.jobs[id].clone();
            if job.status == AgentJobStatus::Completed {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn supervision_history_feedback_and_new_approval_survive_retry() {
    let (service, agent, path) = setup();
    let job = service
        .create_with_controls(&agent, "proposal", "prompt", "proposal", 2, true)
        .await
        .unwrap();
    service
        .approve(&agent, &job.id, job.revision)
        .await
        .unwrap();
    service.start().await.unwrap();
    let done = wait_completed(&service, &job.id).await;
    assert!(service
        .review_output(
            &agent,
            &job.id,
            done.revision,
            JobReviewDecision::ChangesRequested,
            " "
        )
        .await
        .is_err());
    let reviewed = service
        .review_output(
            &agent,
            &job.id,
            done.revision,
            JobReviewDecision::ChangesRequested,
            "Include the test evidence",
        )
        .await
        .unwrap();
    assert!(service
        .review_output(
            &agent,
            &job.id,
            reviewed.revision,
            JobReviewDecision::Accepted,
            ""
        )
        .await
        .is_err());
    let retried = service
        .retry(&agent, &job.id, reviewed.revision, false)
        .await
        .unwrap();
    assert_eq!(retried.status, AgentJobStatus::AwaitingApproval);
    assert!(retried.approved_at_ms.is_none());
    assert_eq!(retried.attempts, reviewed.attempts);
    assert!(job_prompt(&retried).contains("Include the test evidence"));
    service
        .approve(&agent, &job.id, retried.revision)
        .await
        .unwrap();
    let done = wait_completed(&service, &job.id).await;
    assert_eq!(done.attempts.len(), 2);
    assert!(done
        .result
        .as_deref()
        .unwrap()
        .contains("Include the test evidence"));
    assert_eq!(done.attempts[0], reviewed.attempts[0]);
    let accepted = service
        .review_output(
            &agent,
            &job.id,
            done.revision,
            JobReviewDecision::Accepted,
            "Looks good",
        )
        .await
        .unwrap();
    assert!(accepted.validate().is_ok());
    let saved = load_control_plane_snapshot(&ControlPlaneStoreConfig::Json(path.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.jobs[0], accepted);
    service.shutdown().await;
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn supervision_approval_and_review_save_failures_roll_back() {
    let (service, agent, path) = setup();
    let proposal = service
        .create_with_controls(&agent, "proposal", "prompt", "proposal", 2, true)
        .await
        .unwrap();
    let blocked = path.with_extension("blocked");
    std::fs::create_dir(&blocked).unwrap();
    service
        .state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(blocked.clone())));
    assert!(matches!(
        service
            .approve(&agent, &proposal.id, proposal.revision)
            .await,
        Err(JobError::Unavailable(_))
    ));
    assert_eq!(service.state.read().await.jobs[&proposal.id], proposal);
    service
        .state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(path.clone())));
    service
        .approve(&agent, &proposal.id, proposal.revision)
        .await
        .unwrap();
    service.start().await.unwrap();
    let done = wait_completed(&service, &proposal.id).await;
    service
        .state
        .write()
        .await
        .set_control_plane_store(Some(ControlPlaneStoreConfig::Json(blocked.clone())));
    assert!(matches!(
        service
            .review_output(
                &agent,
                &proposal.id,
                done.revision,
                JobReviewDecision::Accepted,
                ""
            )
            .await,
        Err(JobError::Unavailable(_))
    ));
    assert_eq!(service.state.read().await.jobs[&proposal.id], done);
    service.shutdown().await;
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir(blocked);
}

#[tokio::test]
async fn supervision_legacy_output_and_invalid_history_validation() {
    let (service, agent, path) = setup();
    let job = service
        .create(&agent, "legacy", "prompt", "legacy")
        .await
        .unwrap();
    service.start().await.unwrap();
    let done = wait_completed(&service, &job.id).await;
    service.shutdown().await;
    let mut value = serde_json::to_value(&done).unwrap();
    for field in [
        "maxAttempts",
        "requiresApproval",
        "approvedAtMs",
        "attempts",
    ] {
        value.as_object_mut().unwrap().remove(field);
    }
    let mut legacy: AgentJobRecord = serde_json::from_value(value).unwrap();
    assert!(legacy.validate().is_ok());
    assert_eq!(legacy.max_attempts, 3);
    legacy.preserve_legacy_attempt();
    assert_eq!(legacy.attempts[0].result, done.result);
    assert!(legacy.validate().is_ok());
    let mut bad = legacy.clone();
    bad.max_attempts = 0;
    assert!(bad.validate().is_err());
    let mut bad = legacy.clone();
    bad.attempts.push(bad.attempts[0].clone());
    assert!(bad.validate().is_err());
    let mut bad = legacy.clone();
    bad.attempts[0].attempt = 2;
    assert!(bad.validate().is_err());
    let mut bad = legacy.clone();
    bad.attempts[0].status = AgentJobStatus::Queued;
    assert!(bad.validate().is_err());
    let mut bad = legacy.clone();
    bad.attempts[0].review = Some(JobOutputReview {
        decision: JobReviewDecision::ChangesRequested,
        note: " ".into(),
        reviewed_at_ms: bad.updated_at_ms,
    });
    assert!(bad.validate().is_err());
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn supervision_pending_capacity_is_independent_and_cancel_is_revision_guarded() {
    let (service, agent, path) = setup();
    for index in 0..8 {
        service
            .create(&agent, "queued", "prompt", &format!("queued{index}"))
            .await
            .unwrap();
    }
    let proposal = service
        .create_with_controls(&agent, "proposal", "prompt", "proposal", 3, true)
        .await
        .unwrap();
    assert!(matches!(
        service
            .approve(&agent, &proposal.id, proposal.revision)
            .await,
        Err(JobError::Conflict(_))
    ));
    assert_eq!(service.state.read().await.jobs[&proposal.id], proposal);
    assert!(service.cancel(&agent, &proposal.id, 99).await.is_err());
    assert_eq!(
        service
            .cancel(&agent, &proposal.id, proposal.revision)
            .await
            .unwrap()
            .status,
        AgentJobStatus::Cancelled
    );
    let _ = std::fs::remove_file(path);
}
