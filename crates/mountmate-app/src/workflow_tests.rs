//! Exercise user workflows through the real reducer with temporary profiles and unexecuted tasks.
use super::*;

const ID: &str = "audit-mount";

fn fixture() -> (App, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths {
        config_dir: temp.path().join("config"),
        cache_dir: temp.path().join("cache"),
        state_dir: temp.path().join("state"),
        data_dir: temp.path().join("data"),
    };
    let settings = Settings {
        cache_root: paths.cache_dir.clone(),
        auto_check_updates: false,
        auto_show_transfers: false,
        ..Settings::default()
    };
    let server = ServerConfig {
        id: ID.into(),
        name: "Saved connection".into(),
        host: "audit.invalid".into(),
        user: "audit".into(),
        ..ServerConfig::default()
    };
    storage::save_settings(&paths, &settings).unwrap();
    storage::save_servers(&paths, &[server]).unwrap();
    let instance_lock = Arc::new(InstanceLock::try_acquire(&paths.app_instance_lock()).unwrap());
    let command_server =
        Arc::new(AppCommandServer::start(paths.app_command_state(), &Platform, |_| {}).unwrap());
    let (_sender, receiver) = async_channel::unbounded();
    let (mut app, tasks) = App::new(Bootstrap {
        paths,
        instance_lock,
        command_server,
        command_receiver: CommandSubscription(receiver),
        initial_command: AppCommand::ShowMain,
        update_health: None,
    });
    drop(tasks);
    app.mount_observations
        .insert(ID.into(), Ok((MountStatus::Mounted, None)));
    (app, temp)
}

fn snapshot(synced: bool) -> TransferSnapshot {
    TransferSnapshot {
        files: vec![],
        queued: usize::from(!synced),
        uploading: 0,
        queued_bytes: u64::from(!synced),
        transferred_bytes: 0,
        percentage: if synced { 100.0 } else { 0.0 },
        errors: 0,
        out_of_space: false,
        synced,
    }
}

fn observe(app: &mut App, result: Result<TransferSnapshot, String>) {
    let request = app.mount_feedback.start_read(ID);
    drop(app.update(Message::TransfersLoaded(vec![(request, result)])));
}

fn synced_twice(app: &mut App) {
    observe(app, Ok(snapshot(true)));
    observe(app, Ok(snapshot(true)));
    assert!(app.mount_feedback.synced(ID));
}

#[tokio::test(flavor = "multi_thread")]
async fn control_fresh_synced_state_allows_unmount_and_unknown_blocks_it() {
    let (mut app, _temp) = fixture();
    drop(app.start_mount_operation(ID.into(), MountOperation::Unmount));
    assert!(
        !app.busy.contains(ID),
        "No observations should prompt before unmount"
    );
    synced_twice(&mut app);
    drop(app.start_mount_operation(ID.into(), MountOperation::Unmount));
    assert!(
        app.busy.contains(ID),
        "Fresh synchronized observations allow unmount"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn first_transfer_error_must_block_unconfirmed_unmount() {
    let (mut app, _temp) = fixture();
    synced_twice(&mut app);
    observe(&mut app, Err("synthetic RC timeout".into()));
    drop(app.start_mount_operation(ID.into(), MountOperation::Unmount));
    assert!(
        !app.busy.contains(ID),
        "Current transfer status is unknown; unmount must prompt instead of starting immediately"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_confirmation_must_be_consecutive_across_errors() {
    let (mut app, _temp) = fixture();
    observe(&mut app, Ok(snapshot(true)));
    observe(&mut app, Err("synthetic RC timeout".into()));
    observe(&mut app, Ok(snapshot(true)));
    assert!(
        !app.mount_feedback.synced(ID),
        "An error breaks consecutive synchronization evidence"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn late_transfer_reply_must_not_warn_after_successful_unmount() {
    let (mut app, _temp) = fixture();
    observe(&mut app, Ok(snapshot(false)));
    let old_request = app.mount_feedback.start_read(ID);
    drop(app.update(Message::MountFinished {
        id: ID.into(),
        operation: MountOperation::Unmount,
        result: Ok(Some("Synthetic unmount complete".into())),
    }));
    assert_eq!(app.mount_status(ID), Some(MountStatus::Unmounted));
    assert!(app.mount_feedback.snapshot(ID).is_none());
    // A poll that started before unmount finishes afterwards.
    drop(app.update(Message::TransfersLoaded(vec![(
        old_request,
        Ok(snapshot(false)),
    )])));
    assert!(app.mount_feedback.snapshot(ID).is_none());
    drop(app.request_exit());
    assert!(
        !app.exit_confirmation_open,
        "An obsolete poll must not cause an upload warning for an already unmounted mapping"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_center_round_trip_must_preserve_editor_draft() {
    let (mut app, _temp) = fixture();
    app.mount_observations
        .insert(ID.into(), Ok((MountStatus::Unmounted, None)));
    drop(app.update(Message::Edit(ID.into())));
    assert_eq!(app.navigation.current(), Screen::ConnectionEditor);
    drop(app.update(Message::ConnectionFieldChanged(
        ConnectionField::Name,
        "Unsaved user input".into(),
    )));
    assert_eq!(
        app.editor.draft.as_ref().unwrap().name,
        "Unsaved user input"
    );
    drop(app.update(Message::TrayAction(TrayAction::ShowTransfers)));
    assert_eq!(app.navigation.current(), Screen::TransferCenter);
    drop(app.update(Message::CloseTransfers));
    assert_eq!(app.navigation.current(), Screen::ConnectionEditor);
    assert_eq!(
        app.editor.draft.as_ref().unwrap().name,
        "Unsaved user input",
        "Visiting transfers must restore the editor or explicitly confirm discarding its draft"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn old_mount_replies_cannot_contaminate_a_new_mount_or_another_mapping() {
    let (mut app, _temp) = fixture();
    synced_twice(&mut app);
    let old = app.mount_feedback.start_read(ID);
    let sibling = app.mount_feedback.start_read("sibling");
    drop(app.start_mount_operation(ID.into(), MountOperation::Unmount));
    drop(app.update(Message::MountFinished {
        id: ID.into(),
        operation: MountOperation::Unmount,
        result: Ok(Some("Unmounted".into())),
    }));
    // A new successful mount may occur before the old polling worker returns.
    drop(app.update(Message::MountFinished {
        id: ID.into(),
        operation: MountOperation::Mount,
        result: Ok(Some("Mounted".into())),
    }));
    let new = app.mount_feedback.start_read(ID);
    drop(app.update(Message::TransfersLoaded(vec![
        (old.clone(), Ok(snapshot(true))),
        (sibling, Ok(snapshot(false))),
    ])));
    assert!(app.mount_feedback.snapshot(ID).is_none());
    assert!(app.mount_feedback.snapshot("sibling").is_some());
    drop(app.update(Message::CapacitiesLoaded(vec![(
        old,
        Err("obsolete".into()),
    )])));
    assert!(!app.mount_feedback.capacity_failed(ID));
    drop(app.update(Message::TransfersLoaded(vec![(new, Ok(snapshot(false)))])));
    assert_eq!(app.mount_feedback.snapshot(ID).unwrap().queued, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn removed_mapping_rejects_its_outstanding_polls() {
    let (mut app, _temp) = fixture();
    let old = app.mount_feedback.start_read(ID);
    app.servers.clear();
    app.refresh_mount_targets();
    drop(app.update(Message::TransfersLoaded(vec![(
        old.clone(),
        Ok(snapshot(false)),
    )])));
    drop(app.update(Message::CapacitiesLoaded(vec![(
        old,
        Err("obsolete".into()),
    )])));
    assert!(app.mount_feedback.snapshot(ID).is_none());
    assert!(!app.mount_feedback.capacity_failed(ID));
    assert_eq!(app.pending_transfer_counts(), (0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn waiting_unmount_needs_two_new_synced_observations_after_an_error() {
    let (mut app, _temp) = fixture();
    observe(&mut app, Ok(snapshot(false)));
    drop(app.update(Message::UnmountWaitDecision {
        ids: vec![ID.into()],
        choice: UnmountChoice::Wait,
    }));
    observe(&mut app, Ok(snapshot(true)));
    observe(&mut app, Err("timeout".into()));
    assert_eq!(app.pending_transfer_counts(), (0, 1));
    observe(&mut app, Ok(snapshot(true)));
    assert!(app.pending_unmount_after_sync.contains(ID));
    assert!(!app.busy.contains(ID));
    observe(&mut app, Ok(snapshot(true)));
    assert!(!app.pending_unmount_after_sync.contains(ID));
    assert!(app.busy.contains(ID));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_unmount_does_not_schedule_work() {
    let (mut app, _temp) = fixture();
    drop(app.update(Message::UnmountWaitDecision {
        ids: vec![ID.into()],
        choice: UnmountChoice::Cancel,
    }));
    assert!(!app.pending_unmount_after_sync.contains(ID));
    assert!(!app.confirmed_unmounts.contains(ID));
    assert!(!app.busy.contains(ID));
    assert_eq!(app.mount_status(ID), Some(MountStatus::Mounted));
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_preserve_sai_authorization_and_process_its_callbacks() {
    let (mut app, _temp) = fixture();
    drop(app.update(Message::AddConnection));
    drop(app.update(Message::ConnectionSourceChanged(
        ConnectionSource::SaiCluster,
    )));
    drop(app.update(Message::SaiSetup(sai_setup::Message::Start)));
    assert!(app.editor.sai.in_progress());
    drop(app.update(Message::OpenTransfers));
    drop(app.update(Message::AppCommand(AppCommand::ShowTransfers)));
    assert!(app.editor.sai.in_progress());
    assert_eq!(app.navigation.context(), Screen::ConnectionEditor);
    // Cancel while the editor is covered: the callback still belongs to its workflow.
    drop(app.update(Message::SaiSetup(sai_setup::Message::Cancel)));
    assert!(!app.editor.sai.in_progress());
    drop(app.update(Message::CloseTransfers));
    assert_eq!(app.navigation.current(), Screen::ConnectionEditor);
    assert!(app.editor.draft.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_save_completing_behind_transfers_updates_the_return_page() {
    let (mut app, _temp) = fixture();
    app.mount_observations
        .insert(ID.into(), Ok((MountStatus::Unmounted, None)));
    drop(app.update(Message::Edit(ID.into())));
    app.editor_saving = true;
    drop(app.update(Message::OpenTransfers));
    drop(app.update(Message::ConnectionSaved(Ok(ServerMutation {
        servers: app.servers.clone(),
        warning: None,
    }))));
    assert_eq!(app.navigation.current(), Screen::TransferCenter);
    drop(app.update(Message::CloseTransfers));
    assert_eq!(app.navigation.current(), Screen::Connections);
    assert!(app.editor.draft.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_and_command_unmounts_use_the_same_current_transfer_evidence() {
    for message in [
        Message::MountConnection(ID.into(), MountOperation::Unmount),
        Message::AppCommand(AppCommand::UnmountAll),
        Message::BatchUnmountSelected,
    ] {
        let (mut app, _temp) = fixture();
        app.selected_connections.insert(ID.into());
        synced_twice(&mut app);
        observe(&mut app, Err("timeout".into()));
        drop(app.update(message));
        assert!(!app.busy.contains(ID));
        assert_eq!(app.mount_status(ID), Some(MountStatus::Mounted));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_mount_status_does_not_count_as_a_finished_transfer() {
    let (mut app, _temp) = fixture();
    synced_twice(&mut app);
    app.mount_observations
        .insert(ID.into(), Err("process query failed".into()));
    assert_eq!(app.pending_transfer_counts(), (0, 1));
    drop(app.request_exit());
    assert!(app.exit_confirmation_open);
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_path_editing_preserves_names_and_only_locks_the_active_mapping() {
    let (mut app, _temp) = fixture();
    app.servers[0].remote_path = "/active".into();
    app.servers[0]
        .mounts
        .push(mountmate_core::model::MountMapping {
            id: "extra".into(),
            remote_path: "/idle".into(),
            mountpoint: String::new(),
        });
    app.refresh_mount_targets();
    drop(app.update(Message::Edit(ID.into())));
    drop(app.update(Message::RemotePathChanged(
        MappingRow::Primary,
        RemotePathChange::Suffix("blocked".into()),
    )));
    let suffix = "组织/报告\\literal ";
    drop(app.update(Message::RemotePathChanged(
        MappingRow::Additional(0),
        RemotePathChange::Suffix(suffix.into()),
    )));
    let draft = app.editor.draft.as_ref().unwrap();
    assert_eq!(draft.remote_path, "/active");
    assert_eq!(draft.mounts[0].remote_path, format!("/{suffix}"));
}
