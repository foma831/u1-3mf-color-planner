use super::*;
use std::fs::File;
use std::io::Write as _;
use tempfile::tempdir;
use u1_three_mf::{PaintNode, encode_paint_annotation};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Override PartName="/Metadata/project_settings.config" ContentType="application/json"/>
  <Override PartName="/Metadata/model_settings.config" ContentType="application/xml"/>
</Types>"#;

const ROOT_RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

const MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:BambuStudio="http://schemas.bambulab.com/package/2021"
       unit="millimeter">
  <metadata name="Application">Snapmaker Orca 2.3.5</metadata>
  <metadata name="BambuStudio:3mfVersion">1</metadata>
  <metadata name="Title">Full Spectrum candidate</metadata>
  <resources>
    <object id="1" type="model"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
    </mesh></object>
  </resources>
  <build><item objectid="1" printable="1"/></build>
</model>"#;

const MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1">
    <metadata key="name" value="Mixed coupon"/>
    <metadata key="extruder" value="5"/>
  </object>
  <plate>
    <metadata key="plater_id" value="1"/>
    <metadata key="plater_name" value="Coupon"/>
    <metadata key="filament_map_mode" value="Manual"/>
    <metadata key="filament_maps" value="5"/>
    <metadata key="filament_volume_maps" value="9"/>
    <model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="0"/></model_instance>
  </plate>
</config>"#;

fn rgb(red: u8, green: u8, blue: u8) -> RgbColor {
    RgbColor { red, green, blue }
}

fn physical_loadout() -> [U1FullSpectrumPhysicalSlot; 4] {
    let colors = [
        rgb(0x08, 0xab, 0xfb),
        rgb(0xd9, 0x3b, 0x90),
        rgb(0xf9, 0xed, 0x3d),
        rgb(0x91, 0x99, 0xa4),
    ];
    std::array::from_fn(|index| U1FullSpectrumPhysicalSlot {
        toolhead: Toolhead::ALL[index],
        spool_id: format!("spool-{}", index + 1),
        spool_name: format!("Spool {}", index + 1),
        material: Material::Pla,
        color: colors[index],
        profile: FULL_SPECTRUM_PROFILE_NAME.into(),
        setting_id: FULL_SPECTRUM_SETTING_ID.into(),
        filament_id: "1417031127011".into(),
        wildcard_resolved: index == 3,
    })
}

fn compile_context(loadout: &[U1FullSpectrumPhysicalSlot; 4]) -> U1FullSpectrumCompileContext {
    U1FullSpectrumCompileContext {
        calibration_fingerprint: calibration_fingerprint(loadout).unwrap(),
        process: u1_full_spectrum_process_contract(),
        t4_color: Some(loadout[3].color),
    }
}

fn mixed_input(confidence: ColorConfidence) -> U1FullSpectrumRecipeInput {
    U1FullSpectrumRecipeInput {
        logical_id: "requirement-1".into(),
        target_material: Material::Pla,
        target_color: rgb(100, 140, 180),
        recipe: CmyxRecipe::FullSpectrum {
            mode: FullSpectrumMode::Ratio,
            sequence: vec![Toolhead::T1, Toolhead::T1, Toolhead::T2],
        },
        calibration_sample_id: None,
        predicted_color: Some(rgb(90, 130, 170)),
        confidence,
    }
}

fn prepared_artifact() -> U1FullSpectrumPreparedArtifact {
    prepared_artifact_with_calibration_sample(None)
}

fn prepared_artifact_with_calibration_sample(
    sample_id: Option<&str>,
) -> U1FullSpectrumPreparedArtifact {
    let loadout = physical_loadout();
    let calibration_fingerprint = calibration_fingerprint(&loadout).unwrap();
    let calibration_sample_id = sample_id.map(str::to_owned);
    let mut input = mixed_input(ColorConfidence::Calibrated);
    input.calibration_sample_id = calibration_sample_id.clone();
    let recipe_table =
        compile_u1_full_spectrum_recipes(&[input], &compile_context(&loadout)).unwrap();
    let target_filament_id = recipe_table.definitions[0].virtual_filament_id;
    let recipe_fingerprint = recipe_table.definitions[0].recipe_fingerprint.clone();
    let predicted_color = recipe_table.definitions[0].predicted_color;
    let unit = ScopedUnitRef {
        scope_id: "scope-1".into(),
        unit_id: "unit-1".into(),
    };
    U1FullSpectrumPreparedArtifact {
        batch_id: "batch-1".into(),
        file_name: "coupon.3mf".into(),
        loadout,
        calibration_fingerprint,
        process: u1_full_spectrum_process_contract(),
        support: u1_three_mf::SupportInformation {
            enabled: Some(true),
            support_type: Some(u1_three_mf::SupportType::TreeAuto),
            threshold_angle_degrees: Some(20),
            on_build_plate_only: Some(true),
        },
        recipe_table,
        recipe_calibration_sample_ids: calibration_sample_id.iter().cloned().collect(),
        assignments: vec![U1FullSpectrumPreparedAssignment {
            scope_id: "scope-1".into(),
            source_requirement_ids: vec!["requirement-1".into()],
            source_slots: vec![1],
            source_material: Material::Pla,
            source_color: rgb(100, 140, 180),
            target_filament_id,
            recipe_fingerprint: Some(recipe_fingerprint),
            calibration_sample_id,
            predicted_color,
        }],
        plates: vec![U1FullSpectrumPreparedPlate {
            plan_plate_id: "plate-1".into(),
            target_plate_id: 1,
            job_id: "job-1".into(),
            prime_tower: Some(U1FullSpectrumPrimeTower {
                x_mm: 205.9,
                y_mm: 198.9,
            }),
            units: vec![U1FullSpectrumPreparedUnit {
                unit,
                source_unit_id: "source-unit-1".into(),
                source_object_id: 1,
                source_instance_id: 0,
                source_model_path: None,
                source_plate_id: Some("1".into()),
                target_min_x_mm: 10.0,
                target_min_y_mm: 10.0,
                source_to_target_slots: BTreeMap::from([(1, 5)]),
            }],
        }],
    }
}

#[test]
fn full_spectrum_contract_accepts_a_qualified_solid_polymaker_t4_profile() {
    let mut artifact = prepared_artifact();
    artifact.loadout[3].spool_id = "panchroma-basic-black".into();
    artifact.loadout[3].spool_name = "Polymaker Panchroma Basic PLA — Black".into();
    artifact.loadout[3].color = rgb(0x08, 0x0a, 0x0d);
    artifact.loadout[3].profile = POLYMAKER_PLA_PROFILE_NAME.into();
    artifact.loadout[3].setting_id = POLYMAKER_PLA_SETTING_ID.into();
    artifact.loadout[3].filament_id = POLYMAKER_PLA_FILAMENT_ID.into();
    artifact.calibration_fingerprint = calibration_fingerprint(&artifact.loadout).unwrap();

    let (settings, _) = project_settings_bytes(&artifact);

    validate_project_settings_map(&settings, Some(&artifact)).unwrap();
    assert_eq!(settings["enable_support"], Value::String("1".into()));
    assert_eq!(settings["support_type"], Value::String("tree(auto)".into()));
    assert_eq!(
        settings["support_threshold_angle"],
        Value::String("20".into())
    );
    assert_eq!(
        settings["support_on_build_plate_only"],
        Value::String("1".into())
    );
    assert_eq!(
        settings["different_settings_to_system"],
        serde_json::json!([
            "enable_support;support_on_build_plate_only;support_threshold_angle;support_type",
            "",
            "",
            "",
            "",
            ""
        ])
    );
    assert_eq!(
        setting_string_array(&settings, "filament_settings_id").unwrap()[3],
        POLYMAKER_PLA_PROFILE_NAME
    );
}

#[test]
fn heterogeneous_profile_merge_uses_the_full_spectrum_key_contract_without_empty_sentinels() {
    let baseline = BTreeMap::from([
        (
            "chamber_temperature".into(),
            Value::Array(vec![Value::String("0".into())]),
        ),
        (
            "pressure_advance".into(),
            Value::Array(vec![Value::String("0.02".into())]),
        ),
    ]);
    let mut t4 = baseline.clone();
    t4.insert(
        "pressure_advance".into(),
        Value::Array(vec![Value::String("0.04".into())]),
    );
    t4.insert(
        "profile_only_legacy_option".into(),
        Value::Array(vec![Value::String("35".into())]),
    );
    let profiles = vec![baseline.clone(), baseline.clone(), baseline, t4];
    let mut settings = BTreeMap::new();

    merge_full_spectrum_physical_profile_arrays(&mut settings, &profiles).unwrap();

    assert_eq!(
        setting_string_array(&settings, "chamber_temperature").unwrap(),
        vec!["0", "0", "0", "0"]
    );
    assert_eq!(
        setting_string_array(&settings, "pressure_advance").unwrap(),
        vec!["0.02", "0.02", "0.02", "0.04"]
    );
    assert!(!settings.contains_key("profile_only_legacy_option"));
    assert!(settings.values().all(|value| {
        value
            .as_array()
            .is_none_or(|values| values.iter().all(|value| value.as_str() != Some("")))
    }));
}

#[test]
fn heterogeneous_profile_normalizes_legacy_chamber_temperature_and_rejects_conflicts() {
    let mut legacy = BTreeMap::from([(
        "chamber_temperatures".into(),
        Value::Array(vec![Value::String("0".into())]),
    )]);
    normalize_heterogeneous_filament_overlay(&mut legacy).unwrap();
    assert!(!legacy.contains_key("chamber_temperatures"));
    assert_eq!(
        legacy.get("chamber_temperature"),
        Some(&Value::Array(vec![Value::String("0".into())]))
    );

    let mut conflicting = BTreeMap::from([
        (
            "chamber_temperature".into(),
            Value::Array(vec![Value::String("0".into())]),
        ),
        (
            "chamber_temperatures".into(),
            Value::Array(vec![Value::String("45".into())]),
        ),
    ]);
    let error = normalize_heterogeneous_filament_overlay(&mut conflicting).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicting chamber_temperature")
    );
}

fn project_settings_bytes(
    artifact: &U1FullSpectrumPreparedArtifact,
) -> (BTreeMap<String, Value>, Vec<u8>) {
    let mut settings = BTreeMap::from([("layer_height".into(), Value::String("0.08".into()))]);
    apply_u1_full_spectrum_project_patch(&mut settings, artifact).unwrap();
    let mut bytes = serde_json::to_vec_pretty(&settings).unwrap();
    bytes.push(b'\n');
    (settings, bytes)
}

fn write_substrate(path: &Path, project_settings: &[u8], include_stale_entry: bool) {
    write_substrate_with_model(
        path,
        project_settings,
        MODEL.as_bytes(),
        include_stale_entry,
    );
}

fn write_substrate_with_model(
    path: &Path,
    project_settings: &[u8],
    model: &[u8],
    include_stale_entry: bool,
) {
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    let mut entries = vec![
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELATIONSHIPS.as_bytes()),
        ("3D/3dmodel.model", model),
        (PROJECT_SETTINGS_PATH, project_settings),
        (MODEL_SETTINGS_PATH, MODEL_SETTINGS.as_bytes()),
    ];
    if include_stale_entry {
        entries.push(("Metadata/slice_info.config", b"<config/>"));
    }
    for (name, bytes) in entries {
        archive.start_file(name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();
}

#[test]
fn recipe_compilation_is_deterministic_and_native_rows_are_canonical() {
    let loadout = physical_loadout();
    let context = compile_context(&loadout);
    let first =
        compile_u1_full_spectrum_recipes(&[mixed_input(ColorConfidence::Calibrated)], &context)
            .unwrap();
    let second =
        compile_u1_full_spectrum_recipes(&[mixed_input(ColorConfidence::Calibrated)], &context)
            .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.definitions.len(), 1);
    let definition = &first.definitions[0];
    assert_eq!(definition.virtual_filament_id, 5);
    assert!(
        definition
            .serialized
            .starts_with("1,2,1,1,33,0,g,w,m2,z4,xa0,xb0,d0,o0,u")
    );
    assert!(definition.serialized.ends_with(",cm0,112"));
    let parsed = parse_u1_full_spectrum_definitions(&first.serialized_definitions).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].virtual_filament_id, 5);
    assert_eq!(parsed[0].stable_id, definition.stable_id);
    assert_eq!(parsed[0].serialized, definition.serialized);
}

#[test]
fn inherited_paint_state_is_not_a_filament_assignment() {
    let paint = encode_paint_annotation(&PaintNode::Split {
        split_sides: 1,
        special_side: 0,
        children: vec![PaintNode::Leaf { state: 0 }, PaintNode::Leaf { state: 5 }],
    })
    .unwrap();
    let model = format!(r#"<model><triangle paint_color="{paint}"/></model>"#);
    let mut used_ids = BTreeSet::new();

    collect_model_paint_filaments_cancellable(
        std::io::Cursor::new(model.as_bytes()),
        "3D/3dmodel.model",
        &mut used_ids,
        &mut |_| false,
    )
    .unwrap();

    assert_eq!(used_ids, BTreeSet::from([5]));
}

#[test]
fn recipe_compiler_deduplicates_identical_mixes_but_not_logical_targets() {
    let loadout = physical_loadout();
    let context = compile_context(&loadout);
    let mut second = mixed_input(ColorConfidence::Calibrated);
    second.logical_id = "requirement-2".into();
    let table = compile_u1_full_spectrum_recipes(
        &[mixed_input(ColorConfidence::Calibrated), second],
        &context,
    )
    .unwrap();
    assert_eq!(table.definitions.len(), 1);
    assert_eq!(table.targets.len(), 2);
    assert!(
        table
            .targets
            .iter()
            .all(|target| target.target_filament_id == 5)
    );
}

#[test]
fn uncalibrated_black_or_white_t4_mix_is_rejected() {
    let loadout = physical_loadout();
    let mut context = compile_context(&loadout);
    context.t4_color = Some(rgb(0, 0, 0));
    let mut input = mixed_input(ColorConfidence::Nominal);
    input.recipe = CmyxRecipe::FullSpectrum {
        mode: FullSpectrumMode::Cycle,
        sequence: vec![Toolhead::T1, Toolhead::T4],
    };
    let error = compile_u1_full_spectrum_recipes(&[input.clone()], &context).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("uncalibrated near-black or near-white")
    );
    input.confidence = ColorConfidence::Calibrated;
    assert!(compile_u1_full_spectrum_recipes(&[input], &context).is_ok());
}

#[test]
fn calibration_candidate_can_target_an_unmeasured_extreme_t4_without_claiming_provenance() {
    let loadout = physical_loadout();
    let mut context = compile_context(&loadout);
    context.t4_color = Some(rgb(0, 0, 0));
    let mut input = mixed_input(ColorConfidence::Nominal);
    input.recipe = CmyxRecipe::FullSpectrum {
        mode: FullSpectrumMode::Ratio,
        sequence: vec![Toolhead::T1, Toolhead::T4],
    };

    let table = compile_u1_full_spectrum_calibration_candidate_recipes(&[input], &context)
        .expect("an explicit calibration target may consume an unmeasured black T4");

    assert_eq!(table.definitions.len(), 1);
    assert_eq!(table.definitions[0].calibration_sample_id, None);
    assert_eq!(table.targets[0].calibration_sample_id, None);
}

#[test]
fn calibration_candidate_rejects_false_existing_measurement_provenance() {
    let loadout = physical_loadout();
    let context = compile_context(&loadout);
    let mut input = mixed_input(ColorConfidence::Nominal);
    input.calibration_sample_id = Some("already-measured".into());

    let error = compile_u1_full_spectrum_calibration_candidate_recipes(&[input], &context)
        .expect_err("a chart target must never pose as a measured recipe");

    assert!(error.to_string().contains("must not claim"));
}

#[test]
fn native_parser_rejects_noncanonical_or_incomplete_rows() {
    let valid = "1,2,1,1,50,0,g,w,m2,z4,xa0,xb0,d0,o0,u1,cm0,12";
    assert!(parse_u1_full_spectrum_definitions(valid).is_ok());
    assert!(parse_u1_full_spectrum_definitions(&valid.replace(",50,", ",050,")).is_err());
    assert!(parse_u1_full_spectrum_definitions(&valid.replace(",z4,", ",z0,")).is_ok());
    assert!(parse_u1_full_spectrum_definitions(&valid.replace(",u1,", ",u0,")).is_err());
    assert!(parse_u1_full_spectrum_definitions(&format!("{valid};{valid}")).is_err());
}

#[test]
fn project_patch_synchronizes_four_physical_arrays_and_planned_tower() {
    let artifact = prepared_artifact();
    let (settings, _) = project_settings_bytes(&artifact);
    assert_eq!(
        setting_string_array(&settings, "filament_settings_id")
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        setting_string_array(&settings, "filament_ids").unwrap(),
        vec![FULL_SPECTRUM_SETTING_ID; 4]
    );
    assert_eq!(
        setting_string_array(&settings, "filament_type").unwrap(),
        vec!["PLA"; 4]
    );
    assert_eq!(
        setting_string_array(&settings, "wipe_tower_x").unwrap(),
        vec!["205.9"]
    );
    assert_eq!(
        setting_string_array(&settings, "wipe_tower_y").unwrap(),
        vec!["198.9"]
    );
    assert_eq!(
        validate_project_settings_map(&settings, Some(&artifact))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn prime_tower_validation_uses_the_maximum_height_envelope() {
    assert!(validate_prime_tower_anchor(205.9, 198.9).is_ok());
    assert!(validate_prime_tower_anchor(215.0, 200.0).is_err());
    assert!(validate_prime_tower_anchor(0.5, 1.0).is_err());
}

#[test]
fn qualification_writer_is_deterministic_and_rewrites_plate_maps() {
    let directory = tempdir().unwrap();
    let artifact = prepared_artifact();
    let (_, settings) = project_settings_bytes(&artifact);
    let substrate = directory.path().join("substrate.3mf");
    write_substrate(&substrate, &settings, false);
    let first = write_u1_full_spectrum_qualification_candidate(
        &substrate,
        &directory.path().join("first.3mf"),
        &settings,
        &artifact,
    )
    .unwrap();
    let second = write_u1_full_spectrum_qualification_candidate(
        &substrate,
        &directory.path().join("second.3mf"),
        &settings,
        &artifact,
    )
    .unwrap();
    assert_eq!(first.sha256, second.sha256);
    assert_eq!(first.byte_size, second.byte_size);
    assert!(first.validation.valid);
    let validation = validate_u1_full_spectrum_candidate(&first.path, Some(&artifact)).unwrap();
    assert!(validation.valid, "{:?}", validation.issues);
}

#[test]
fn recovery_validation_rejects_a_standalone_valid_foreign_geometry_package() {
    let directory = tempdir().unwrap();
    let artifact = prepared_artifact();
    let (_, settings) = project_settings_bytes(&artifact);
    let substrate = directory.path().join("current-substrate.3mf");
    let foreign_substrate = directory.path().join("foreign-substrate.3mf");
    write_substrate(&substrate, &settings, false);
    let foreign_model = MODEL.replace(
        "<vertex x=\"10\" y=\"0\" z=\"0\"/>",
        "<vertex x=\"11\" y=\"0\" z=\"0\"/>",
    );
    write_substrate_with_model(
        &foreign_substrate,
        &settings,
        foreign_model.as_bytes(),
        false,
    );
    let current = write_u1_full_spectrum_qualification_candidate(
        &substrate,
        &directory.path().join("current.3mf"),
        &settings,
        &artifact,
    )
    .unwrap();
    let foreign = write_u1_full_spectrum_qualification_candidate(
        &foreign_substrate,
        &directory.path().join("foreign.3mf"),
        &settings,
        &artifact,
    )
    .unwrap();

    assert!(
        validate_u1_full_spectrum_candidate(&foreign.path, Some(&artifact))
            .unwrap()
            .valid
    );
    assert!(
        validate_u1_full_spectrum_output_against_substrate(
            &current.path,
            &substrate,
            &settings,
            &artifact,
        )
        .unwrap()
        .valid
    );
    let error = validate_u1_full_spectrum_output_against_substrate(
        &foreign.path,
        &substrate,
        &settings,
        &artifact,
    )
    .unwrap_err();
    assert!(error.to_string().contains("normalized substrate"));
}

#[test]
fn qualification_validation_preserves_recipe_calibration_provenance() {
    let directory = tempdir().unwrap();
    let artifact = prepared_artifact_with_calibration_sample(Some("measured-coupon-r1"));
    let (_, settings) = project_settings_bytes(&artifact);
    let substrate = directory.path().join("calibrated-substrate.3mf");
    write_substrate(&substrate, &settings, false);

    let written = write_u1_full_spectrum_qualification_candidate(
        &substrate,
        &directory.path().join("calibrated-candidate.3mf"),
        &settings,
        &artifact,
    )
    .unwrap();

    assert!(written.validation.valid, "{:?}", written.validation.issues);
    assert_eq!(
        written.validation.recipe_calibration_sample_ids,
        vec!["measured-coupon-r1".to_owned()]
    );
    let evidence = serde_json::to_value(&written.validation).unwrap();
    assert_eq!(
        evidence["recipeCalibrationSampleIds"][0],
        "measured-coupon-r1"
    );
    assert_eq!(
        artifact.recipe_table.definitions[0]
            .calibration_sample_id
            .as_deref(),
        Some("measured-coupon-r1")
    );
}

#[test]
fn qualification_writer_rejects_stale_slicer_artifacts() {
    let directory = tempdir().unwrap();
    let artifact = prepared_artifact();
    let (_, settings) = project_settings_bytes(&artifact);
    let substrate = directory.path().join("stale-substrate.3mf");
    write_substrate(&substrate, &settings, true);
    let error = write_u1_full_spectrum_qualification_candidate(
        &substrate,
        &directory.path().join("candidate.3mf"),
        &settings,
        &artifact,
    )
    .unwrap_err();
    assert!(error.to_string().contains("stale-sensitive"));
    assert!(!directory.path().join("candidate.3mf").exists());
}

#[test]
fn cancellation_before_publication_drops_every_staged_candidate() {
    let directory = tempdir().unwrap();
    let artifact = prepared_artifact();
    let (_, settings) = project_settings_bytes(&artifact);
    let substrate = directory.path().join("substrate.3mf");
    let destination = directory.path().join("cancelled.3mf");
    write_substrate(&substrate, &settings, false);

    let staged = stage_u1_full_spectrum_candidate_internal_cancellable(
        &substrate,
        &destination,
        &settings,
        &artifact,
        &mut |_| false,
    )
    .unwrap();
    assert!(!destination.exists());

    let error = publish_staged_full_spectrum_artifacts_cancellable(
        vec![PendingFullSpectrumPublication {
            batch_id: artifact.batch_id.clone(),
            file_name: artifact.file_name.clone(),
            plate_count: artifact.plates.len(),
            target_plate_ids: vec!["plate-1".into()],
            source_unit_ids: vec!["source-unit-1".into()],
            staged,
        }],
        &mut |checkpoint| checkpoint == U1FullSpectrumCancellationCheckpoint::BeforePublication,
    )
    .expect_err("cancellation must win before the publication boundary");

    let error_message = error.to_string();
    assert!(matches!(
        error,
        U1FullSpectrumError::Cancelled {
            checkpoint: U1FullSpectrumCancellationCheckpoint::BeforePublication
        }
    ));
    assert!(error_message.starts_with(U1_FULL_SPECTRUM_CANCELLATION_ERROR_PREFIX));
    assert!(!destination.exists());
    let remaining = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        remaining,
        vec![substrate.file_name().unwrap().to_os_string()]
    );
}

#[test]
fn cancellable_production_api_can_stop_before_capability_inspection() {
    let input = PlanningInput {
        scopes: Vec::new(),
        inventory: Vec::new(),
        current_toolheads: u1_planner::CurrentToolheadState::default(),
        config: u1_planner::PlannerConfig::with_cmy_setup(u1_planner::CmySetup {
            cyan_spool_id: "c".into(),
            magenta_spool_id: "m".into(),
            yellow_spool_id: "y".into(),
            default_t4_spool_id: None,
        }),
    };
    let result = PlanningResult {
        scope_options: Vec::new(),
        jobs: Vec::new(),
        plates: Vec::new(),
        batches: Vec::new(),
        t4_swap_count: 0,
        a1_spool_change_count: 0,
        final_toolheads: u1_planner::CurrentToolheadState::default(),
        warnings: Vec::new(),
        errors: Vec::new(),
    };
    let output = tempdir().unwrap();
    let callback_called = std::cell::Cell::new(false);

    let error = convert_u1_full_spectrum_with_substrate_builder_cancellable(
        Path::new("/missing/Snapmaker Orca.app"),
        &input,
        &result,
        output.path(),
        |_, _| {
            callback_called.set(true);
            Err(U1FullSpectrumError::Plan(
                "substrate callback must not run after cancellation".into(),
            ))
        },
        |checkpoint| checkpoint == U1FullSpectrumCancellationCheckpoint::ConversionStart,
    )
    .expect_err("an initial cancellation must bypass capability inspection");

    assert!(matches!(
        error,
        U1FullSpectrumError::Cancelled {
            checkpoint: U1FullSpectrumCancellationCheckpoint::ConversionStart
        }
    ));
    assert!(!callback_called.get());
    assert!(std::fs::read_dir(output.path()).unwrap().next().is_none());
}

#[test]
fn embedded_full_spectrum_qualification_documents_are_hash_bound() {
    assert!(full_spectrum_qualification_documents_are_valid(
        U1_FULL_SPECTRUM_QUALIFICATION_RECORD,
        U1_FULL_SPECTRUM_QUALIFICATION_REPORT,
    ));

    let mut changed_report = U1_FULL_SPECTRUM_QUALIFICATION_REPORT.to_vec();
    changed_report.push(b' ');
    assert!(!full_spectrum_qualification_documents_are_valid(
        U1_FULL_SPECTRUM_QUALIFICATION_RECORD,
        &changed_report,
    ));

    let changed_record = String::from_utf8(U1_FULL_SPECTRUM_QUALIFICATION_RECORD.to_vec())
        .unwrap()
        .replace(
            "\"writerQualificationScope\": \"native_project_structure_and_gui_round_trip\"",
            "\"writerQualificationScope\": \"physical_color_accuracy\"",
        );
    assert!(!full_spectrum_qualification_documents_are_valid(
        changed_record.as_bytes(),
        U1_FULL_SPECTRUM_QUALIFICATION_REPORT,
    ));
}

#[test]
fn production_gate_accepts_the_exact_gui_qualified_installation() {
    let application = Path::new("/Applications/Snapmaker Orca.app");
    if !application.is_dir() {
        return;
    }
    let installation = inspect_macos_application(application).unwrap();
    assert!(full_spectrum_qualification_bundle_is_valid(&installation));
    let capability = inspect_u1_full_spectrum_macos_application(application).unwrap();
    assert!(capability.installation_supported());
    assert!(capability.qualification_candidate_available);
    assert!(capability.qualification_evidence_valid);
    assert!(capability.conversion_available);
    assert_eq!(capability.status, U1FullSpectrumCapabilityStatus::Qualified);
}
