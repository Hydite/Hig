use super::*;

#[test]
fn persisted_absolute_path_labels_are_platform_neutral() {
    for path in [
        "/var/lib/hig/recovery",
        "C:\\Users\\operator\\Hig Vault",
        "d:/recovery/vault",
        "\\\\server\\share\\hig",
    ] {
        validate_persisted_absolute_path_label(path).unwrap();
    }

    for path in ["", "relative/path", "C:relative", "\\rooted", "bad\0path"] {
        assert!(validate_persisted_absolute_path_label(path).is_err());
    }
}

#[test]
fn unknown_vault_and_nested_catalog_schemas_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    init_recovery_vault(Some(&vault), Vec::new()).unwrap();

    let valid_config = load_vault_config(&vault).unwrap();
    let mut unknown_config = valid_config.clone();
    unknown_config.schema = VAULT_SCHEMA + 1;
    write_checked_json(&vault_config_path(&vault), &unknown_config).unwrap();
    assert!(
        load_vault_config(&vault)
            .unwrap_err()
            .to_string()
            .contains("vault schema")
    );
    write_checked_json(&vault_config_path(&vault), &valid_config).unwrap();

    let valid_catalog = load_catalog(&vault).unwrap();
    let mut unknown_catalog = valid_catalog.clone();
    unknown_catalog.schema = CATALOG_SCHEMA + 1;
    write_checked_json(&catalog_path(&vault), &unknown_catalog).unwrap();
    assert!(
        load_catalog(&vault)
            .unwrap_err()
            .to_string()
            .contains("catalog schema")
    );
    write_checked_json(&catalog_path(&vault), &valid_catalog).unwrap();

    let repository_id = [7_u8; 16];
    let repository_key = hex::encode(repository_id);
    let now = now_unix_ns();
    let mut nested_catalog = valid_catalog;
    nested_catalog.repositories.insert(
        repository_key,
        RecoveryRegistration {
            schema: VAULT_SCHEMA + 1,
            registration_id: [3_u8; 16],
            repository_id,
            created_unix_ns: now,
            updated_unix_ns: now,
            source_paths: vec![temp.path().join("source").display().to_string()],
            recovery_points: BTreeMap::new(),
            tombstones: Vec::new(),
        },
    );
    write_checked_json(&catalog_path(&vault), &nested_catalog).unwrap();
    assert!(
        load_catalog(&vault)
            .unwrap_err()
            .to_string()
            .contains("registration schema")
    );
}

#[test]
fn catalog_identity_and_durability_invariants_fail_closed() {
    let repository_id = [9_u8; 16];
    let commit_id: RepositoryObjectId =
        serde_json::from_value(serde_json::Value::String("05".repeat(32))).unwrap();
    let point_id = commit_id.to_hex();
    let now = now_unix_ns();
    let point = RecoveryPoint {
        schema: VAULT_SCHEMA,
        recovery_point_id: point_id.clone(),
        commit_id,
        ref_name: format!("tags/recovery/{point_id}"),
        captured_unix_ns: now,
        last_verified_unix_ns: now,
        reachable_objects: 1,
        stored_objects_written: 1,
        stored_bytes_written: 1,
        durability: RecoveryDurability::Protected,
        replicas: Vec::new(),
        pinned: false,
        state: RecoveryPointState::Available,
    };
    let registration = RecoveryRegistration {
        schema: VAULT_SCHEMA,
        registration_id: [4_u8; 16],
        repository_id,
        created_unix_ns: now,
        updated_unix_ns: now,
        source_paths: vec![std::env::temp_dir().join("source").display().to_string()],
        recovery_points: BTreeMap::from([(point_id, point)]),
        tombstones: Vec::new(),
    };
    let catalog = RecoveryCatalog {
        schema: CATALOG_SCHEMA,
        generation: 1,
        repositories: BTreeMap::from([(hex::encode(repository_id), registration)]),
    };
    assert!(
        validate_catalog(&catalog)
            .unwrap_err()
            .to_string()
            .contains("verified replica set")
    );

    let mut wrong_key = catalog;
    wrong_key.repositories = BTreeMap::from([(
        "00000000000000000000000000000000".into(),
        wrong_key.repositories.into_values().next().unwrap(),
    )]);
    assert!(
        validate_catalog(&wrong_key)
            .unwrap_err()
            .to_string()
            .contains("repository identity")
    );
}
