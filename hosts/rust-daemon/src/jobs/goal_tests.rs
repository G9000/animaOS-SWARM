use super::*;

#[tokio::test]
async fn goals_last_slot_is_atomic_and_cancel_releases_reservation() {
    let (service, agent, path) = setup();
    let goal = service
        .create_goal("goal", "objective", "goal", 1)
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        service.create_with_goal(
            &agent,
            "one",
            "prompt",
            "one",
            3,
            false,
            Some(&goal.record.id)
        ),
        service.create_with_goal(
            &agent,
            "two",
            "prompt",
            "two",
            3,
            false,
            Some(&goal.record.id)
        )
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let job = first.or(second).unwrap();
    let view = service.list_goals().await.unwrap().remove(0);
    assert_eq!(
        (
            view.consumed_attempts,
            view.reserved_attempts,
            view.remaining_attempts
        ),
        (0, 1, 0)
    );
    service.cancel(&agent, &job.id, job.revision).await.unwrap();
    assert_eq!(service.list_goals().await.unwrap()[0].remaining_attempts, 1);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn goals_pause_survives_restore_and_completion_requires_accepted_output() {
    let (service, agent, path) = setup();
    let goal = service
        .create_goal("goal", "objective", "goal", 2)
        .await
        .unwrap();
    let job = service
        .create_with_goal(
            &agent,
            "one",
            "prompt",
            "one",
            2,
            false,
            Some(&goal.record.id),
        )
        .await
        .unwrap();
    let goal = service
        .change_goal_status(&goal.record.id, goal.record.revision, GoalStatus::Paused)
        .await
        .unwrap();
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
        .change_goal_status(&goal.record.id, goal.record.revision, GoalStatus::Completed)
        .await
        .is_err());
    let goal = service
        .change_goal_status(&goal.record.id, goal.record.revision, GoalStatus::Active)
        .await
        .unwrap();
    let done = wait_completed(&service, &job.id).await;
    assert!(service
        .change_goal_status(&goal.record.id, goal.record.revision, GoalStatus::Completed)
        .await
        .is_err());
    service
        .review_output(
            &agent,
            &job.id,
            done.revision,
            JobReviewDecision::Accepted,
            "",
        )
        .await
        .unwrap();
    let goal = service
        .change_goal_status(&goal.record.id, goal.record.revision, GoalStatus::Completed)
        .await
        .unwrap();
    assert_eq!((goal.consumed_attempts, goal.accepted_outputs), (1, 1));
    assert!(service
        .change_goal_status(&goal.record.id, goal.record.revision, GoalStatus::Active)
        .await
        .is_err());
    assert!(service
        .create_with_goal(
            &agent,
            "two",
            "prompt",
            "two",
            3,
            true,
            Some(&goal.record.id)
        )
        .await
        .is_err());
    service.shutdown().await;
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn goals_failed_save_rolls_back_status_and_job_reservations() {
    let (service, agent, path) = setup();
    let goal = service
        .create_goal("goal", "objective", "goal", 1)
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
            .change_goal_status(&goal.record.id, goal.record.revision, GoalStatus::Paused)
            .await,
        Err(JobError::Unavailable(_))
    ));
    assert_eq!(service.list_goals().await.unwrap()[0], goal);
    assert!(matches!(
        service
            .create_with_goal(
                &agent,
                "one",
                "prompt",
                "one",
                3,
                false,
                Some(&goal.record.id)
            )
            .await,
        Err(JobError::Unavailable(_))
    ));
    assert_eq!(service.list_goals().await.unwrap()[0], goal);
    assert!(service.goal_jobs(&goal.record.id).await.unwrap().is_empty());
    assert!(matches!(
        service.create_goal("other", "objective", "other", 1).await,
        Err(JobError::Unavailable(_))
    ));
    assert_eq!(service.list_goals().await.unwrap().len(), 1);
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir(blocked);
}

#[tokio::test]
async fn goals_uncertain_attempts_remain_consumed_and_proposals_cannot_bypass_budget() {
    let (service, agent, path) = setup();
    let goal = service
        .create_goal("goal", "objective", "goal", 1)
        .await
        .unwrap();
    let job = service
        .create_with_goal(
            &agent,
            "one",
            "prompt",
            "one",
            3,
            false,
            Some(&goal.record.id),
        )
        .await
        .unwrap();
    {
        let mut state = service.state.write().await;
        let job = state.jobs.get_mut(&job.id).unwrap();
        job.status = AgentJobStatus::Running;
        job.attempt = 1;
        job.started_at_ms = Some(job.updated_at_ms);
        review(job, "uncertain");
    }
    let current = service.list(&agent).await.unwrap().remove(0);
    assert!(service
        .retry(&agent, &job.id, current.revision, true)
        .await
        .is_err());
    let proposal = service
        .create_with_goal(
            &agent,
            "proposal",
            "prompt",
            "proposal",
            3,
            true,
            Some(&goal.record.id),
        )
        .await
        .unwrap();
    assert!(service
        .approve(&agent, &proposal.id, proposal.revision)
        .await
        .is_err());
    let view = service.list_goals().await.unwrap().remove(0);
    assert_eq!(
        (
            view.consumed_attempts,
            view.reserved_attempts,
            view.remaining_attempts
        ),
        (1, 0, 0)
    );
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn goals_validate_links_budget_completion_and_idempotency() {
    let (service, agent, path) = setup();
    let first = service
        .create_goal("goal", "objective", "first", 1)
        .await
        .unwrap();
    assert_eq!(
        service
            .create_goal("goal", "objective", "first", 1)
            .await
            .unwrap(),
        first
    );
    assert!(service
        .create_goal("goal", "different", "first", 1)
        .await
        .is_err());
    let second = service
        .create_goal("second", "objective", "second", 2)
        .await
        .unwrap();
    let job = service
        .create_with_goal(
            &agent,
            "one",
            "prompt",
            "one",
            3,
            false,
            Some(&first.record.id),
        )
        .await
        .unwrap();
    assert!(service
        .create_with_goal(
            &agent,
            "one",
            "prompt",
            "one",
            3,
            false,
            Some(&second.record.id)
        )
        .await
        .is_err());
    assert!(service
        .create_with_goal(
            &agent,
            "missing",
            "prompt",
            "missing",
            3,
            true,
            Some("missing")
        )
        .await
        .is_err());
    assert!(validate_goals(&[first.record.clone()], &[job.clone()]).is_ok());
    assert!(validate_goals(&[], &[job.clone()]).is_err());
    let mut extra = job.clone();
    extra.id = "extra".into();
    assert!(validate_goals(&[first.record.clone()], &[job.clone(), extra]).is_err());
    let mut completed = first.record.clone();
    completed.status = GoalStatus::Completed;
    assert!(validate_goals(&[completed], &[job.clone()]).is_err());
    assert!(validate_goals(
        &[first.record.clone(), first.record.clone()],
        &[job.clone()]
    )
    .is_err());
    let mut legacy = serde_json::to_value(&job).unwrap();
    legacy.as_object_mut().unwrap().remove("goalId");
    let legacy: AgentJobRecord = serde_json::from_value(legacy).unwrap();
    assert!(legacy.goal_id.is_none());
    assert!(validate_goals(&[], &[legacy]).is_ok());
    let _ = std::fs::remove_file(path);
}
