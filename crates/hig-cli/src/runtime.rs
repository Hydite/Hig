use crate::cli::{
    CacheCommand, DaemonCommand, ProjectCommand, ProjectPolicyCommand, ReportMode, SessionCommand,
    TaskCommand,
};
use hig_core::{
    ArchiveFormat, DaemonMode, DaemonRequest, DaemonResponse, EncryptionMode, JobKeyMaterial,
    KdfProfile, PackAuthMode, PackJobRequest, PackOptions, PackReport, PackResponseMode,
    ProjectMode, ProjectRegistration, SerializablePackOptions, SolidMode, SpeedMode,
    cache_writer_available, daemon_socket_path, daemon_status, default_session_ttl, derive_key,
    derive_session_binding, discover_project, init_project, pack, random_bytes, request_daemon,
    resolve_project_cache_dir, run_daemon_server, stop_daemon,
};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Duration, Instant};

pub(crate) fn handle_project_init(
    dir: &Path,
    cache_dir: Option<PathBuf>,
    excludes: Vec<String>,
) -> anyhow::Result<()> {
    let config = init_project(dir, cache_dir, excludes)?;
    let root = dir.canonicalize()?;
    let cache = resolve_project_cache_dir(&root, &config);
    ensure_daemon(&cache, default_session_ttl(None))?;
    let response = request_daemon(
        &cache,
        DaemonRequest::ProjectRegister(ProjectRegistration {
            root: root.clone(),
            config: config.clone(),
        }),
    )?;
    match response {
        Some(DaemonResponse::ProjectRegistered(status)) => {
            println!(
                "project: initialized root={} cache_dir={} project_id={} files={} generation={} state={:?}",
                root.display(),
                cache.display(),
                hex::encode(config.project_id),
                status.files,
                status.generation,
                status.snapshot_validity
            );
            Ok(())
        }
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not register project"),
    }
}

pub(crate) fn project_registration(
    dir: &Path,
    cache_override: Option<&Path>,
) -> anyhow::Result<(PathBuf, PathBuf, hig_core::ProjectConfig)> {
    let (root, config) = discover_project(dir)?.ok_or_else(|| {
        anyhow::anyhow!(
            "project is not initialized; run `hig init {}`",
            dir.display()
        )
    })?;
    let cache = resolve_project_cache_dir(&root, &config);
    if let Some(override_path) = cache_override {
        let expected = cache.canonicalize().unwrap_or(cache.clone());
        let requested = override_path
            .canonicalize()
            .unwrap_or_else(|_| override_path.to_path_buf());
        anyhow::ensure!(
            expected == requested,
            "--cache-dir does not match the initialized project cache"
        );
    }
    Ok((root, cache, config))
}

pub(crate) fn register_project(
    root: &Path,
    cache: &Path,
    config: &hig_core::ProjectConfig,
) -> anyhow::Result<hig_core::ProjectStatusReport> {
    ensure_daemon(cache, default_session_ttl(None))?;
    match request_daemon(
        cache,
        DaemonRequest::ProjectRegister(ProjectRegistration {
            root: root.to_path_buf(),
            config: config.clone(),
        }),
    )? {
        Some(DaemonResponse::ProjectRegistered(status)) => Ok(status),
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not register project"),
    }
}

pub(crate) fn handle_project_watch(
    dir: &Path,
    cache_override: Option<PathBuf>,
    verbose: bool,
) -> anyhow::Result<()> {
    let (root, cache, config) = project_registration(dir, cache_override.as_deref())?;
    let mut status = register_project(&root, &cache, &config)?;
    println!(
        "watch: active root={} backend={} generation={} files={}",
        root.display(),
        status.watcher_backend,
        status.generation,
        status.files
    );
    loop {
        std::thread::sleep(Duration::from_millis(250));
        match request_daemon(
            &cache,
            DaemonRequest::ProjectStatus {
                project_id: config.project_id,
            },
        )? {
            Some(DaemonResponse::ProjectStatus(next)) => {
                if verbose && next.event_sequence != status.event_sequence {
                    println!(
                        "watch: generation={} events={} pending={} dirty_files={} state={:?}",
                        next.generation,
                        next.event_sequence,
                        next.pending_events,
                        next.dirty_files,
                        next.snapshot_validity
                    );
                }
                status = next;
            }
            Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
            _ => anyhow::bail!("watch daemon became unavailable"),
        }
    }
}

pub(crate) fn handle_project(command: ProjectCommand) -> anyhow::Result<()> {
    match command {
        ProjectCommand::Status { dir, json } => {
            let (root, cache, config) = project_registration(&dir, None)?;
            register_project(&root, &cache, &config)?;
            match request_daemon(
                &cache,
                DaemonRequest::ProjectStatus {
                    project_id: config.project_id,
                },
            )? {
                Some(DaemonResponse::ProjectStatus(status)) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&status)?);
                    } else {
                        println!(
                            "project: root={} state={:?} generation={} files={} pending={} dirty_files={} backend={} prepared_bytes={}",
                            status.root,
                            status.snapshot_validity,
                            status.generation,
                            status.files,
                            status.pending_events,
                            status.dirty_files,
                            status.watcher_backend,
                            status.prepared_bytes
                        );
                    }
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return project status"),
            }
        }
        ProjectCommand::Rebuild { dir, wait: _ } => {
            let (root, cache, config) = project_registration(&dir, None)?;
            register_project(&root, &cache, &config)?;
            match request_daemon(
                &cache,
                DaemonRequest::ProjectRebuild {
                    project_id: config.project_id,
                },
            )? {
                Some(DaemonResponse::ProjectStatus(status)) => {
                    println!(
                        "project: rebuilt generation={} files={} state={:?}",
                        status.generation, status.files, status.snapshot_validity
                    );
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not rebuild project"),
            }
        }
        ProjectCommand::Policy { command } => handle_project_policy(command),
    }
}

fn handle_project_policy(command: ProjectPolicyCommand) -> anyhow::Result<()> {
    match command {
        ProjectPolicyCommand::Show { dir, json } => {
            let (root, _, config) = project_registration(&dir, None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&config.snapshot_policy)?);
            } else {
                println!(
                    "project-policy: root={} enabled={} quiescence_ms={} periodic_interval_ms={} max_pending_events={} max_pending_files={} resource_enabled={} min_available_memory_bytes={} resume_available_memory_bytes={} resource_poll_interval_ms={}",
                    root.display(),
                    config.snapshot_policy.enabled,
                    config.snapshot_policy.quiescence_ms,
                    config.snapshot_policy.periodic_interval_ms,
                    config.snapshot_policy.max_pending_events,
                    config.snapshot_policy.max_pending_files,
                    config.snapshot_policy.resource.enabled,
                    config.snapshot_policy.resource.min_available_memory_bytes,
                    config
                        .snapshot_policy
                        .resource
                        .resume_available_memory_bytes,
                    config.snapshot_policy.resource.poll_interval_ms
                );
            }
        }
        ProjectPolicyCommand::Set {
            dir,
            enabled,
            quiescence_ms,
            periodic_interval_ms,
            max_pending_events,
            max_pending_files,
            resource_enabled,
            min_available_memory_bytes,
            resume_available_memory_bytes,
            resource_poll_interval_ms,
            json,
        } => {
            let (root, cache, config) = project_registration(&dir, None)?;
            let mut policy = config.snapshot_policy.clone();
            if let Some(value) = enabled {
                policy.enabled = value;
            }
            if let Some(value) = quiescence_ms {
                policy.quiescence_ms = value;
            }
            if let Some(value) = periodic_interval_ms {
                policy.periodic_interval_ms = value;
            }
            if let Some(value) = max_pending_events {
                policy.max_pending_events = value;
            }
            if let Some(value) = max_pending_files {
                policy.max_pending_files = value;
            }
            if let Some(value) = resource_enabled {
                policy.resource.enabled = value;
            }
            if let Some(value) = min_available_memory_bytes {
                policy.resource.min_available_memory_bytes = value;
            }
            if let Some(value) = resume_available_memory_bytes {
                policy.resource.resume_available_memory_bytes = value;
            }
            if let Some(value) = resource_poll_interval_ms {
                policy.resource.poll_interval_ms = value;
            }
            ensure_daemon(&cache, default_session_ttl(None))?;
            register_project(&root, &cache, &config)?;
            match request_daemon(
                &cache,
                DaemonRequest::ProjectPolicyUpdate {
                    project_id: config.project_id,
                    policy,
                },
            )? {
                Some(DaemonResponse::ProjectStatus(status)) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&status)?);
                    } else {
                        println!(
                            "project-policy: updated root={} policy_schema={} paused={} state={:?}",
                            root.display(),
                            status.policy_schema,
                            status.snapshot_paused,
                            status.snapshot_validity
                        );
                    }
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not update project policy"),
            }
        }
    }
    Ok(())
}

pub(crate) fn handle_cache(command: CacheCommand) -> anyhow::Result<()> {
    let (cache_dir, request) = match command {
        CacheCommand::Status { cache_dir } => (
            cache_dir.unwrap_or_else(default_cache_dir),
            DaemonRequest::CacheStatus,
        ),
        CacheCommand::Gc { cache_dir, dry_run } => (
            cache_dir.unwrap_or_else(default_cache_dir),
            DaemonRequest::CacheGc { dry_run },
        ),
        CacheCommand::Compact { cache_dir, dry_run } => (
            cache_dir.unwrap_or_else(default_cache_dir),
            DaemonRequest::CacheCompact { dry_run },
        ),
    };
    ensure_daemon(&cache_dir, default_session_ttl(None))?;
    match request_daemon(&cache_dir, request)? {
        Some(DaemonResponse::CacheMaintenance(report)) => {
            println!(
                "cache: total_bytes={} budget_bytes={} files={} removable_bytes={} removed_bytes={} compacted_bytes={} generation={} journal_bytes={} journal_entries={} journal_replayed_entries={} journal_compacted_entries={} journal_dirty_record_estimate={} journal_compact_recommended={} journal_estimated_reclaimed_bytes={} last_compact_unix_ns={} dry_run={}",
                report.total_bytes,
                report.budget_bytes,
                report.files,
                report.removable_bytes,
                report.removed_bytes,
                report.compacted_bytes,
                report.generation,
                report.journal_bytes,
                report.journal_entries,
                report.journal_replayed_entries,
                report.journal_compacted_entries,
                report.journal_dirty_record_estimate,
                report.journal_compact_recommended,
                report.journal_estimated_reclaimed_bytes,
                report.last_compact_unix_ns,
                report.dry_run
            );
            Ok(())
        }
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not return cache maintenance status"),
    }
}

pub(crate) fn handle_task(command: TaskCommand) -> anyhow::Result<()> {
    match command {
        TaskCommand::List {
            cache_dir,
            include_completed,
        } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            match request_daemon(&cache_dir, DaemonRequest::TaskList { include_completed })? {
                Some(DaemonResponse::TaskList(tasks)) => {
                    println!("{}", serde_json::to_string_pretty(&tasks)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return task list"),
            }
        }
        TaskCommand::Status { task_id, cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let task_id = parse_task_id(&task_id)?;
            match request_daemon(&cache_dir, DaemonRequest::TaskStatus { task_id })? {
                Some(DaemonResponse::TaskStatus(status)) => {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return task status"),
            }
        }
        TaskCommand::Cancel { task_id, cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let task_id = parse_task_id(&task_id)?;
            match request_daemon(&cache_dir, DaemonRequest::TaskCancel { task_id })? {
                Some(DaemonResponse::TaskStatus(status)) => {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not cancel task"),
            }
        }
        TaskCommand::Result { task_id, cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let task_id = parse_task_id(&task_id)?;
            match request_daemon(&cache_dir, DaemonRequest::TaskResult { task_id })? {
                Some(DaemonResponse::TaskResult(result)) => {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                    Ok(())
                }
                Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
                _ => anyhow::bail!("daemon did not return task result"),
            }
        }
    }
}

pub(crate) fn parse_task_id(value: &str) -> anyhow::Result<[u8; 16]> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("task id must be a 16-byte hex string"))
}

pub(crate) fn validate_pack_encryption_args(
    encryption: EncryptionMode,
    password: Option<&str>,
    use_session: bool,
) -> anyhow::Result<()> {
    match (encryption, password, use_session) {
        (EncryptionMode::Password, Some(value), _) if !value.is_empty() => Ok(()),
        (EncryptionMode::Password, None, true) => Ok(()),
        (EncryptionMode::Password, _, _) => {
            anyhow::bail!("--encryption password requires --password")
        }
        (EncryptionMode::None, None, false) => Ok(()),
        (EncryptionMode::None, _, _) => {
            anyhow::bail!("--password and --use-session cannot be used with --encryption none")
        }
    }
}

pub(crate) fn pack_with_daemon_policy(
    mut options: PackOptions,
    report_mode: ReportMode,
) -> anyhow::Result<PackReport> {
    if options.pipeline.project_mode != ProjectMode::Off
        && options.cache_dir.is_none()
        && let Some((root, config)) = discover_project(&options.input_dir)?
    {
        options.cache_dir = Some(resolve_project_cache_dir(&root, &config));
    }
    if options.pipeline.project_mode == ProjectMode::Required
        && (options.format == ArchiveFormat::HigV1
            || options.pipeline.daemon_mode == DaemonMode::Off
            || !options.use_cache)
    {
        anyhow::bail!("--project required needs HIGV2, cache, and an active daemon");
    }
    if options.format == ArchiveFormat::HigV1
        || options.pipeline.daemon_mode == DaemonMode::Off
        || !options.use_cache
    {
        return pack(options);
    }
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| options.input_dir.join(".hig-cache"));
    fs::create_dir_all(&cache_dir)?;
    let connect_started = Instant::now();
    let daemon_ready = ensure_daemon(&cache_dir, default_session_ttl(None));
    if let Err(error) = daemon_ready {
        if options.pipeline.daemon_mode == DaemonMode::Required || options.use_session {
            return Err(error);
        }
        return pack(options);
    }
    let daemon_connect_us = connect_started.elapsed().as_micros() as u64;
    let kdf = options.kdf_profile.params();
    let binding = derive_session_binding(&cache_dir, options.kdf_profile, &kdf, options.encryption);
    let mut ephemeral_key = None;
    let auth_mode = if options.encryption == EncryptionMode::None {
        PackAuthMode::None
    } else if options.use_session {
        PackAuthMode::UseSession
    } else if options.password.is_some() {
        let salt = random_bytes::<16>();
        let password = options
            .password
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("password encryption requires a password"))?;
        let key = derive_key(password, &salt, &kdf)?;
        ephemeral_key = Some(JobKeyMaterial { key, salt });
        PackAuthMode::PreferSessionOrJobKey
    } else {
        PackAuthMode::UseSession
    };
    let request = DaemonRequest::SubmitTask(hig_core::TaskSubmitRequest {
        request: hig_core::TaskRequest::Pack(PackJobRequest {
            options: SerializablePackOptions::from_pack(&options),
            binding_fingerprint: Some(binding.fingerprint),
            ephemeral_key,
            auth_mode,
            response_mode: response_mode_for_report(report_mode),
        }),
    });
    let socket_started = Instant::now();
    let response = request_daemon(&cache_dir, request);
    let pack_roundtrip_us = socket_started.elapsed().as_micros() as u64;
    match response {
        Ok(Some(DaemonResponse::TaskAccepted(status))) => {
            let task_id = status.task_id;
            let result = wait_for_task_result(&cache_dir, task_id)?;
            let hig_core::TaskResult::Pack { report } = result else {
                anyhow::bail!("daemon task did not return a pack result");
            };
            let mut report = *report;
            report.timings_us.daemon_connect_us = daemon_connect_us;
            report.timings_us.socket_request_us = pack_roundtrip_us;
            report.timings_us.socket_pack_roundtrip_us = pack_roundtrip_us;
            Ok(report)
        }
        Err(error) if options.pipeline.daemon_mode == DaemonMode::Auto && !options.use_session => {
            if cache_writer_available(&cache_dir)? {
                eprintln!("hig: daemon exited, falling back to standalone: {error}");
                pack(options)
            } else {
                Err(error)
            }
        }
        Ok(Some(DaemonResponse::Error { code, message })) => {
            anyhow::bail!("daemon {:?}: {}", code, message)
        }
        Ok(_) => anyhow::bail!("daemon returned an unexpected pack response"),
        Err(error) => Err(error),
    }
}

pub(crate) fn wait_for_task_result(
    cache_dir: &Path,
    task_id: [u8; 16],
) -> anyhow::Result<hig_core::TaskResult> {
    loop {
        match request_daemon(cache_dir, DaemonRequest::TaskResult { task_id }) {
            Ok(Some(DaemonResponse::TaskResult(result))) => return Ok(result),
            Ok(Some(DaemonResponse::Error { message, .. })) if message.contains("not ready") => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(Some(DaemonResponse::Error { message, .. })) => anyhow::bail!(message),
            Ok(_) => anyhow::bail!("daemon returned an unexpected task result response"),
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn response_mode_for_report(report_mode: ReportMode) -> PackResponseMode {
    match report_mode {
        ReportMode::Short | ReportMode::Quiet => PackResponseMode::Summary,
        ReportMode::Verbose | ReportMode::Json => PackResponseMode::Full,
    }
}

pub(crate) fn effective_kdf_profile(speed: SpeedMode, requested: Option<KdfProfile>) -> KdfProfile {
    requested.unwrap_or(match speed {
        SpeedMode::Balanced => KdfProfile::Secure,
        SpeedMode::Fastest => KdfProfile::Interactive,
    })
}

pub(crate) fn effective_solid(speed: SpeedMode, requested: SolidMode) -> SolidMode {
    match speed {
        SpeedMode::Balanced => requested,
        SpeedMode::Fastest => SolidMode::Off,
    }
}

pub(crate) fn handle_session(command: SessionCommand) -> anyhow::Result<()> {
    match command {
        SessionCommand::Unlock {
            password,
            password_stdin,
            cache_dir,
            ttl_secs,
            kdf_profile,
        } => {
            let password = if password_stdin {
                let mut value = String::new();
                std::io::stdin()
                    .take(64 * 1024 + 1)
                    .read_to_string(&mut value)?;
                anyhow::ensure!(value.len() <= 64 * 1024, "password exceeds stdin limit");
                while value.ends_with(['\n', '\r']) {
                    value.pop();
                }
                anyhow::ensure!(!value.is_empty(), "password from stdin is empty");
                value
            } else {
                password
                    .ok_or_else(|| anyhow::anyhow!("--password or --password-stdin is required"))?
            };
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            fs::create_dir_all(&cache_dir)?;
            let ttl_secs = default_session_ttl(ttl_secs);
            let started = Instant::now();
            ensure_daemon(&cache_dir, ttl_secs)?;
            unlock_session_for_cache(&cache_dir, &password, kdf_profile, ttl_secs)?;
            let status = daemon_status(&cache_dir)?;
            println!(
                "session: unlocked cache_dir={} ttl_secs={} age_secs={} kdf_ms={}",
                cache_dir.display(),
                ttl_secs,
                status.session_age_secs,
                started.elapsed().as_millis()
            );
            Ok(())
        }
        SessionCommand::Status { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let status = daemon_status(&cache_dir)?;
            if status.session_active {
                println!(
                    "session: active cache_dir={} age_secs={} ttl_secs={}",
                    cache_dir.display(),
                    status.session_age_secs,
                    status.ttl_secs
                );
            } else {
                println!("session: inactive cache_dir={}", cache_dir.display());
            }
            Ok(())
        }
        SessionCommand::Clear { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let cleared = matches!(
                request_daemon(&cache_dir, DaemonRequest::ClearSession)?,
                Some(DaemonResponse::SessionCleared)
            );
            println!(
                "session: {} cache_dir={}",
                if cleared { "cleared" } else { "inactive" },
                cache_dir.display()
            );
            Ok(())
        }
    }
}

pub(crate) fn handle_daemon(command: DaemonCommand) -> anyhow::Result<()> {
    match command {
        DaemonCommand::Start {
            cache_dir,
            ttl_secs,
        } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            fs::create_dir_all(&cache_dir)?;
            let ttl_secs = default_session_ttl(ttl_secs);
            ensure_daemon(&cache_dir, ttl_secs)?;
            let status = daemon_status(&cache_dir)?;
            println!(
                "daemon: started cache_dir={} active={} ttl_secs={}",
                cache_dir.display(),
                status.active,
                ttl_secs
            );
            Ok(())
        }
        DaemonCommand::Status { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let status = daemon_status(&cache_dir)?;
            if status.active {
                println!(
                    "daemon: active cache_dir={} age_secs={} uptime_secs={} ttl_secs={} jobs_completed={} active_jobs={} queued_jobs={} cache_open_count={} session_active={} journal_bytes={} watched_projects={} project_ready_count={} project_pending_events={}",
                    cache_dir.display(),
                    status.age_secs,
                    status.uptime_secs,
                    status.ttl_secs,
                    status.jobs_completed,
                    status.active_jobs,
                    status.queued_jobs,
                    status.cache_open_count,
                    status.session_active,
                    status.journal_bytes,
                    status.watched_projects,
                    status.project_ready_count,
                    status.project_pending_events
                );
            } else {
                println!("daemon: inactive cache_dir={}", cache_dir.display());
            }
            Ok(())
        }
        DaemonCommand::Stop { cache_dir } => {
            let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
            let stopped = stop_daemon(&cache_dir)?;
            println!(
                "daemon: {} cache_dir={}",
                if stopped { "stopped" } else { "inactive" },
                cache_dir.display()
            );
            Ok(())
        }
        DaemonCommand::Serve {
            cache_dir,
            ttl_secs,
        } => run_daemon_server(&cache_dir, ttl_secs),
    }
}

pub(crate) fn ensure_daemon(cache_dir: &Path, ttl_secs: u64) -> anyhow::Result<()> {
    if daemon_status(cache_dir)?.active {
        return Ok(());
    }
    fs::create_dir_all(cache_dir)?;
    let socket = daemon_socket_path(cache_dir);
    if socket.exists() {
        let _ = fs::remove_file(&socket);
    }
    let mut child = ProcessCommand::new(std::env::current_exe()?)
        .arg("daemon")
        .arg("serve")
        .arg("--cache-dir")
        .arg(cache_dir)
        .arg("--ttl-secs")
        .arg(ttl_secs.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    for _ in 0..100 {
        if socket.exists() && daemon_status(cache_dir)?.active {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            anyhow::bail!("Hig daemon exited during startup");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    anyhow::bail!("Hig daemon did not create its socket")
}

pub(crate) fn unlock_session_for_cache(
    cache_dir: &Path,
    password: &str,
    kdf_profile: KdfProfile,
    ttl_secs: u64,
) -> anyhow::Result<()> {
    fs::create_dir_all(cache_dir)?;
    ensure_daemon(cache_dir, ttl_secs)?;
    let kdf = kdf_profile.params();
    let binding = derive_session_binding(cache_dir, kdf_profile, &kdf, EncryptionMode::Password);
    let salt = match request_daemon(
        cache_dir,
        DaemonRequest::UnlockChallenge {
            binding: binding.clone(),
        },
    )? {
        Some(DaemonResponse::UnlockChallenge { salt }) => salt,
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not provide an unlock challenge"),
    };
    let mut key = derive_key(password, &salt, &kdf)?;
    let response = request_daemon(
        cache_dir,
        DaemonRequest::InstallSessionKey {
            binding,
            key,
            salt,
            ttl_secs,
        },
    );
    use zeroize::Zeroize;
    key.zeroize();
    match response? {
        Some(DaemonResponse::SessionInstalled) => Ok(()),
        Some(DaemonResponse::Error { message, .. }) => anyhow::bail!(message),
        _ => anyhow::bail!("daemon did not install the session key"),
    }
}

pub(crate) fn default_cache_dir() -> PathBuf {
    PathBuf::from(".hig-cache")
}
