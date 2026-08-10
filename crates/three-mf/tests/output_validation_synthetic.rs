use std::io::{Cursor, Write};

use tempfile::NamedTempFile;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use u1_three_mf::{
    OutputIssueCode, OutputValidationPolicy, OutputValidationReport, StaleArtifactDisposition,
    StaleArtifactKind, StaleArtifactPolicy, StructuralOutputValidator, classify_stale_entry,
    validate_output_reader,
};

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Override PartName="/Metadata/project_settings.config" ContentType="application/json"/>
  <Override PartName="/Metadata/model_settings.config" ContentType="application/xml"/>
</Types>"#;

const SNAPMAKER_ORCA_GUI_SAVE_CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Default Extension="png" ContentType="image/png"/>
</Types>"#;

const ROOT_RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
  <Relationship Target="/Auxiliaries/.thumbnails/thumbnail_3mf.png" Id="thumbnail" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
</Relationships>"#;

const MAIN_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
  <resources>
    <object id="1" type="model"><mesh><vertices/><triangles/></mesh></object>
  </resources>
  <build><item objectid="1"/></build>
</model>"#;

const PROJECT_SETTINGS: &str = r##"{
  "printer_model": "Snapmaker U1",
  "printer_settings_id": "Snapmaker U1 0.4 nozzle",
  "print_settings_id": "0.08 mm Full Spectrum @U1",
  "nozzle_diameter": ["0.4", "0.4", "0.4", "0.4"],
  "filament_settings_id": ["Cyan", "Magenta", "Yellow", "Grey"],
  "filament_colour": ["#00FFFF", "#FF00FF", "#FFFF00", "#808080"],
  "filament_type": ["PLA", "PLA", "PLA", "PLA"]
}"##;

const MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1"><metadata key="name" value="Part"/></object>
  <plate><metadata key="plater_id" value="1"/></plate>
</config>"#;

const GUI_SAVE_MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1"><metadata key="name" value="Part"/></object>
  <plate>
    <metadata key="plater_id" value="1"/>
    <metadata key="thumbnail_file" value="Metadata/plate_1.png"/>
    <metadata key="thumbnail_no_light_file" value="Metadata/plate_no_light_1.png"/>
    <metadata key="top_file" value="Metadata/top_1.png"/>
    <metadata key="pick_file" value="Metadata/pick_1.png"/>
  </plate>
</config>"#;

const BAMBU_GUI_SAVE_MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1">
    <metadata key="name" value="Part"/>
    <metadata face_count="12"/>
  </object>
  <plate>
    <metadata key="plater_id" value="1"/>
    <metadata key="thumbnail_file" value="Metadata/plate_1.png"/>
  </plate>
</config>"#;

const BAMBU_CUT_INFORMATION: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<objects>
  <object id="1"><cut_id id="0" check_sum="1" connectors_cnt="0"/></object>
</objects>"#;

const BAMBU_EMPTY_PLACEHOLDER_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US"
       xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:BambuStudio="http://schemas.bambulab.com/package/2021"
       xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
       requiredextensions="p">
  <metadata name="BambuStudio:3mfVersion">1</metadata>
  <resources></resources>
  <build/>
</model>"#;

fn valid_entries() -> Vec<(String, Vec<u8>)> {
    vec![
        (
            "[Content_Types].xml".into(),
            CONTENT_TYPES.as_bytes().to_vec(),
        ),
        ("_rels/.rels".into(), ROOT_RELATIONSHIPS.as_bytes().to_vec()),
        ("3D/3dmodel.model".into(), MAIN_MODEL.as_bytes().to_vec()),
        (
            "Metadata/project_settings.config".into(),
            PROJECT_SETTINGS.as_bytes().to_vec(),
        ),
        (
            "Metadata/model_settings.config".into(),
            MODEL_SETTINGS.as_bytes().to_vec(),
        ),
        (
            "Auxiliaries/.thumbnails/thumbnail_3mf.png".into(),
            b"synthetic-png".to_vec(),
        ),
    ]
}

fn snapmaker_orca_2_3_5_gui_save_entries() -> Vec<(String, Vec<u8>)> {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "[Content_Types].xml",
        SNAPMAKER_ORCA_GUI_SAVE_CONTENT_TYPES.as_bytes().to_vec(),
    );
    replace(
        &mut entries,
        "Metadata/model_settings.config",
        GUI_SAVE_MODEL_SETTINGS.as_bytes().to_vec(),
    );
    entries.extend([
        (
            "Metadata/slice_info.config".into(),
            b"<config><header/></config>".to_vec(),
        ),
        ("Metadata/plate_1.json".into(), b"{}".to_vec()),
        ("Metadata/plate_1.png".into(), b"plate".to_vec()),
        ("Metadata/plate_1_small.png".into(), b"small-plate".to_vec()),
        (
            "Metadata/plate_no_light_1.png".into(),
            b"plate-no-light".to_vec(),
        ),
        ("Metadata/top_1.png".into(), b"top".to_vec()),
        ("Metadata/pick_1.png".into(), b"pick".to_vec()),
    ]);
    entries
}

fn bambu_studio_2_2_0_85_gui_save_entries() -> Vec<(String, Vec<u8>)> {
    let mut entries = snapmaker_orca_2_3_5_gui_save_entries();
    replace(
        &mut entries,
        "Metadata/model_settings.config",
        BAMBU_GUI_SAVE_MODEL_SETTINGS.as_bytes().to_vec(),
    );
    entries.extend([
        (
            "Metadata/cut_information.xml".into(),
            BAMBU_CUT_INFORMATION.as_bytes().to_vec(),
        ),
        (
            "3D/_rels/3dmodel.model.rels".into(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
              <Relationship Target="/3D/Objects/object_2.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
            </Relationships>"#
                .as_bytes()
                .to_vec(),
        ),
        (
            "3D/Objects/object_2.model".into(),
            BAMBU_EMPTY_PLACEHOLDER_MODEL.as_bytes().to_vec(),
        ),
    ]);
    entries
}

fn snapmaker_orca_gui_save_with_empty_unused_model() -> Vec<(String, Vec<u8>)> {
    let mut entries = snapmaker_orca_2_3_5_gui_save_entries();
    entries.extend([
        (
            "3D/_rels/3dmodel.model.rels".into(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
              <Relationship Target="/3D/Objects/lower leg_2211.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
            </Relationships>"#
                .as_bytes()
                .to_vec(),
        ),
        (
            "3D/Objects/lower leg_2211.model".into(),
            BAMBU_EMPTY_PLACEHOLDER_MODEL.as_bytes().to_vec(),
        ),
    ]);
    entries
}

fn write_archive(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn replace(entries: &mut [(String, Vec<u8>)], name: &str, contents: impl Into<Vec<u8>>) {
    entries
        .iter_mut()
        .find(|(candidate, _)| candidate == name)
        .unwrap()
        .1 = contents.into();
}

fn issue_codes(report: &OutputValidationReport) -> Vec<OutputIssueCode> {
    report.issues.iter().map(|issue| issue.code).collect()
}

#[test]
fn valid_unsliced_package_is_publishable_serializable_and_deterministic() {
    let bytes = write_archive(&valid_entries());
    let policy = OutputValidationPolicy::strict_unsliced();
    let validator = StructuralOutputValidator::new(policy.clone());
    assert_eq!(validator.policy(), &policy);
    let first = validator.validate_reader(Cursor::new(bytes.clone()));
    let second = validate_output_reader(Cursor::new(bytes.clone()), &policy);

    assert!(first.is_valid, "{:#?}", first.issues);
    assert_eq!(first, second);
    assert_eq!(first.statistics.entry_count, 6);
    assert_eq!(first.statistics.crc_checked_entry_count, 6);
    assert_eq!(first.stale_entries.len(), 1);
    assert_eq!(first.stale_entries[0].kind, StaleArtifactKind::Thumbnail);
    assert_eq!(
        first.stale_entries[0].disposition,
        StaleArtifactDisposition::RequireRelationship
    );
    first.ensure_publishable().unwrap();
    assert!(
        serde_json::to_value(&first).unwrap()["isValid"]
            .as_bool()
            .unwrap()
    );

    let mut staged = NamedTempFile::new().unwrap();
    staged.write_all(&bytes).unwrap();
    staged.flush().unwrap();
    let from_staged_path = validator.validate_path(staged.path()).unwrap();
    assert_eq!(from_staged_path, first);
}

#[test]
fn snapmaker_orca_2_3_5_gui_save_policy_accepts_only_known_regenerated_parts() {
    let bytes = write_archive(&snapmaker_orca_2_3_5_gui_save_entries());
    let strict = validate_output_reader(
        Cursor::new(bytes.clone()),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(!strict.is_valid);
    assert!(issue_codes(&strict).contains(&OutputIssueCode::MissingContentTypeDeclaration));
    assert!(issue_codes(&strict).contains(&OutputIssueCode::ForbiddenSlicedArtifact));
    assert!(issue_codes(&strict).contains(&OutputIssueCode::StaleDerivedArtifact));

    let policy = OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save([1]);
    let report = validate_output_reader(Cursor::new(bytes), &policy);

    assert!(report.is_valid, "unexpected issues: {:#?}", report.issues);
    assert!(report.stale_entries.iter().all(|entry| {
        entry.kind == StaleArtifactKind::Thumbnail
            || entry.disposition == StaleArtifactDisposition::Preserve
    }));
}

#[test]
fn snapmaker_orca_policy_accepts_only_provably_empty_unused_model_relationships() {
    let entries = snapmaker_orca_gui_save_with_empty_unused_model();
    let bytes = write_archive(&entries);

    let strict = validate_output_reader(
        Cursor::new(bytes.clone()),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(!strict.is_valid);
    assert!(issue_codes(&strict).contains(&OutputIssueCode::UnusedProductionModelRelationship));

    let snapmaker = validate_output_reader(
        Cursor::new(bytes),
        &OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save([1]),
    );
    assert!(
        snapmaker.is_valid,
        "unexpected issues: {:#?}",
        snapmaker.issues
    );

    let mut nonempty_entries = entries;
    replace(
        &mut nonempty_entries,
        "3D/Objects/lower leg_2211.model",
        BAMBU_EMPTY_PLACEHOLDER_MODEL
            .replace(
                "<resources></resources>",
                "<resources><object id=\"2\" type=\"model\"><mesh><vertices/><triangles/></mesh></object></resources>",
            )
            .into_bytes(),
    );
    let nonempty = validate_output_reader(
        Cursor::new(write_archive(&nonempty_entries)),
        &OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save([1]),
    );
    assert!(issue_codes(&nonempty).contains(&OutputIssueCode::UnusedProductionModelRelationship));
}

#[test]
fn gui_save_policy_rejects_unknown_plate_parts_checksums_and_embedded_presets() {
    let mut entries = snapmaker_orca_2_3_5_gui_save_entries();
    entries.extend([
        ("Metadata/plate_2.json".into(), b"{}".to_vec()),
        ("Metadata/top_2.png".into(), b"unexpected-preview".to_vec()),
        ("Metadata/plate_1.bgcode".into(), b"toolpath".to_vec()),
        ("Metadata/archive.sha256".into(), b"checksum".to_vec()),
        ("Metadata/filament_settings_1.config".into(), b"{}".to_vec()),
    ]);

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save([1]),
    );

    assert!(!report.is_valid);
    assert!(report.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::ForbiddenSlicedArtifact
            && issue.entry.as_deref() == Some("Metadata/plate_1.bgcode")
    }));
    assert!(report.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::UnexpectedGuiSaveArtifact
            && issue.entry.as_deref() == Some("Metadata/archive.sha256")
    }));
    assert!(report.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::ForbiddenEmbeddedPreset
            && issue.entry.as_deref() == Some("Metadata/filament_settings_1.config")
    }));
    for entry in ["Metadata/plate_2.json", "Metadata/top_2.png"] {
        assert!(report.issues.iter().any(|issue| {
            issue.code == OutputIssueCode::UnexpectedGuiSaveArtifact
                && issue.entry.as_deref() == Some(entry)
        }));
    }
}

#[test]
fn bambu_studio_2_2_0_85_policy_accepts_only_observed_safe_gui_artifacts() {
    let bytes = write_archive(&bambu_studio_2_2_0_85_gui_save_entries());

    let strict = validate_output_reader(
        Cursor::new(bytes.clone()),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(!strict.is_valid);
    assert!(issue_codes(&strict).contains(&OutputIssueCode::InvalidModelSettings));
    assert!(issue_codes(&strict).contains(&OutputIssueCode::UnusedProductionModelRelationship));

    let snapmaker = validate_output_reader(
        Cursor::new(bytes.clone()),
        &OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save([1]),
    );
    assert!(!snapmaker.is_valid);
    assert!(snapmaker.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::UnexpectedGuiSaveArtifact
            && issue.entry.as_deref() == Some("Metadata/cut_information.xml")
            && issue.message.contains("Snapmaker Orca 2.3.5")
    }));

    let bambu = validate_output_reader(
        Cursor::new(bytes),
        &OutputValidationPolicy::bambu_studio_2_2_0_85_gui_save([1]),
    );
    assert!(bambu.is_valid, "unexpected issues: {:#?}", bambu.issues);
}

#[test]
fn bambu_gui_policy_rejects_malformed_cut_metadata_and_nonempty_unused_models() {
    let mut malformed_cut_entries = bambu_studio_2_2_0_85_gui_save_entries();
    replace(
        &mut malformed_cut_entries,
        "Metadata/cut_information.xml",
        BAMBU_CUT_INFORMATION
            .replace("connectors_cnt=\"0\"", "connectors_cnt=\"1\"")
            .into_bytes(),
    );
    let malformed_cut = validate_output_reader(
        Cursor::new(write_archive(&malformed_cut_entries)),
        &OutputValidationPolicy::bambu_studio_2_2_0_85_gui_save([1]),
    );
    assert!(malformed_cut.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::UnexpectedGuiSaveArtifact
            && issue.entry.as_deref() == Some("Metadata/cut_information.xml")
    }));

    let mut malformed_metadata_entries = bambu_studio_2_2_0_85_gui_save_entries();
    replace(
        &mut malformed_metadata_entries,
        "Metadata/model_settings.config",
        BAMBU_GUI_SAVE_MODEL_SETTINGS
            .replace(
                "<metadata face_count=\"12\"/>",
                "<metadata value=\"unkeyed\"/>",
            )
            .into_bytes(),
    );
    let malformed_metadata = validate_output_reader(
        Cursor::new(write_archive(&malformed_metadata_entries)),
        &OutputValidationPolicy::bambu_studio_2_2_0_85_gui_save([1]),
    );
    assert!(issue_codes(&malformed_metadata).contains(&OutputIssueCode::InvalidModelSettings));

    let mut nonempty_placeholder_entries = bambu_studio_2_2_0_85_gui_save_entries();
    replace(
        &mut nonempty_placeholder_entries,
        "3D/Objects/object_2.model",
        BAMBU_EMPTY_PLACEHOLDER_MODEL
            .replace(
                "<resources></resources>",
                "<resources><object id=\"2\" type=\"model\"><mesh><vertices/><triangles/></mesh></object></resources>",
            )
            .into_bytes(),
    );
    let nonempty_placeholder = validate_output_reader(
        Cursor::new(write_archive(&nonempty_placeholder_entries)),
        &OutputValidationPolicy::bambu_studio_2_2_0_85_gui_save([1]),
    );
    assert!(
        issue_codes(&nonempty_placeholder)
            .contains(&OutputIssueCode::UnusedProductionModelRelationship)
    );
}

#[test]
fn gui_save_policy_rejects_malformed_regenerated_slice_metadata() {
    let mut entries = snapmaker_orca_2_3_5_gui_save_entries();
    replace(
        &mut entries,
        "Metadata/slice_info.config",
        b"<config>".to_vec(),
    );

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save([1]),
    );

    assert!(!report.is_valid);
    assert!(report.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::InvalidXml
            && issue.entry.as_deref() == Some("Metadata/slice_info.config")
    }));
}

#[test]
fn invalid_package_reports_relationship_settings_stale_and_thumbnail_failures() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "_rels/.rels",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Target="/3D/3dmodel.model" Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
          <Relationship Target="/Metadata/missing.xml" Id="missing" Type="metadata"/>
        </Relationships>"#
            .as_bytes()
            .to_vec(),
    );
    replace(
        &mut entries,
        "Metadata/project_settings.config",
        b"{}".to_vec(),
    );
    replace(
        &mut entries,
        "Metadata/model_settings.config",
        b"<config><metadata value=\"missing-key\"/></config>".to_vec(),
    );
    entries.extend([
        ("Metadata/plate_1.gcode".into(), b"G1 X1".to_vec()),
        ("Metadata/plate_1.gcode.md5".into(), b"deadbeef".to_vec()),
        ("Metadata/slice_info.config".into(), b"<config/>".to_vec()),
        ("Metadata/plate_1.json".into(), b"{}".to_vec()),
        ("Metadata/top_1.png".into(), b"stale-preview".to_vec()),
        ("Metadata/broken.xml".into(), b"<broken>".to_vec()),
    ]);

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    let codes = issue_codes(&report);

    assert!(!report.is_valid);
    assert!(codes.contains(&OutputIssueCode::MissingRelationshipTarget));
    assert!(codes.contains(&OutputIssueCode::InvalidProjectSettings));
    assert!(codes.contains(&OutputIssueCode::InvalidModelSettings));
    assert!(codes.contains(&OutputIssueCode::InvalidXml));
    assert!(codes.contains(&OutputIssueCode::ForbiddenSlicedArtifact));
    assert!(codes.contains(&OutputIssueCode::StaleDerivedArtifact));
    assert!(codes.contains(&OutputIssueCode::DanglingThumbnail));
    let blocked = report.ensure_publishable().unwrap_err();
    assert!(blocked.error_count >= 7);
}

#[test]
fn semantic_xml_elements_must_use_the_expected_namespace_and_hierarchy() {
    let mut content_entries = valid_entries();
    replace(
        &mut content_entries,
        "[Content_Types].xml",
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
          <wrapper><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></wrapper>
        </Types>"#
            .as_bytes()
            .to_vec(),
    );
    let content_report = validate_output_reader(
        Cursor::new(write_archive(&content_entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(issue_codes(&content_report).contains(&OutputIssueCode::InvalidContentTypes));

    let mut relationship_entries = valid_entries();
    replace(
        &mut relationship_entries,
        "_rels/.rels",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <wrapper><Relationship Target="/3D/3dmodel.model" Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></wrapper>
        </Relationships>"#
            .as_bytes()
            .to_vec(),
    );
    let relationship_report = validate_output_reader(
        Cursor::new(write_archive(&relationship_entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(issue_codes(&relationship_report).contains(&OutputIssueCode::InvalidRelationships));

    let mut model_entries = valid_entries();
    replace(
        &mut model_entries,
        "3D/3dmodel.model",
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
          <object id="1" type="model"/>
          <resources/>
          <build/>
        </model>"#
            .as_bytes()
            .to_vec(),
    );
    let model_report = validate_output_reader(
        Cursor::new(write_archive(&model_entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(issue_codes(&model_report).contains(&OutputIssueCode::InvalidXml));
}

#[test]
fn crc_corruption_and_unsafe_paths_fail_closed() {
    let mut entries = valid_entries();
    entries.push(("../outside.xml".into(), b"<outside/>".to_vec()));
    let mut bytes = write_archive(&entries);
    let marker = b"Snapmaker U1";
    let offset = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("stored project settings marker");
    bytes[offset] = b'X';

    let report = validate_output_reader(
        Cursor::new(bytes),
        &OutputValidationPolicy::strict_unsliced(),
    );
    let codes = issue_codes(&report);

    assert!(!report.is_valid);
    assert!(codes.contains(&OutputIssueCode::ZipCrcMismatch));
    assert!(codes.contains(&OutputIssueCode::UnsafeEntryPath));
}

#[test]
fn equivalent_and_prefix_derived_opc_part_names_are_rejected() {
    let mut equivalent_entries = valid_entries();
    equivalent_entries.push((
        "metadata/PROJECT_SETTINGS.CONFIG".into(),
        PROJECT_SETTINGS.as_bytes().to_vec(),
    ));
    let equivalent_report = validate_output_reader(
        Cursor::new(write_archive(&equivalent_entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(issue_codes(&equivalent_report).contains(&OutputIssueCode::EquivalentEntryName));

    let mut prefix_entries = valid_entries();
    prefix_entries.push(("Metadata".into(), b"part".to_vec()));
    let prefix_report = validate_output_reader(
        Cursor::new(write_archive(&prefix_entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(issue_codes(&prefix_report).contains(&OutputIssueCode::PartNameDerivationConflict));
}

#[test]
fn missing_model_content_type_is_a_structured_error() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "[Content_Types].xml",
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
          <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
          <Default Extension="png" ContentType="image/png"/>
        </Types>"#
            .as_bytes()
            .to_vec(),
    );

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );

    assert!(!report.is_valid);
    assert!(issue_codes(&report).contains(&OutputIssueCode::MissingContentTypeDeclaration));
}

#[test]
fn relationship_and_content_type_define_model_parts_independent_of_filename() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "_rels/.rels",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Target="/payload.bin" Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
        </Relationships>"#
            .as_bytes()
            .to_vec(),
    );
    replace(
        &mut entries,
        "[Content_Types].xml",
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
          <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
          <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
          <Default Extension="png" ContentType="image/png"/>
          <Override PartName="/payload.bin" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
          <Override PartName="/Metadata/project_settings.config" ContentType="application/json"/>
          <Override PartName="/Metadata/model_settings.config" ContentType="application/xml"/>
        </Types>"#
            .as_bytes()
            .to_vec(),
    );
    let model_entry = entries
        .iter_mut()
        .find(|(name, _)| name == "3D/3dmodel.model")
        .unwrap();
    model_entry.0 = "payload.bin".into();
    model_entry.1 = b"<not-model/>".to_vec();

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );

    assert!(!report.is_valid);
    assert!(report.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::InvalidModel && issue.entry.as_deref() == Some("payload.bin")
    }));
}

#[test]
fn stale_classifier_distinguishes_toolpath_checksums_and_stage_a_slice_metadata() {
    let checksum = classify_stale_entry("Metadata/plate_1.gcode.md5").unwrap();
    assert_eq!(checksum.kind, StaleArtifactKind::ToolpathChecksum);
    assert_eq!(checksum.disposition, StaleArtifactDisposition::Reject);

    let slice_header = classify_stale_entry("Metadata/slice_info.config").unwrap();
    assert_eq!(slice_header.kind, StaleArtifactKind::SliceMetadata);
    assert_eq!(slice_header.disposition, StaleArtifactDisposition::Reject);

    let sliced = StaleArtifactPolicy::sliced()
        .classify("Metadata/plate_1.gcode")
        .unwrap();
    assert_eq!(sliced.disposition, StaleArtifactDisposition::Preserve);
}

#[test]
fn stale_artifact_references_embedded_in_settings_are_blocking() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "Metadata/project_settings.config",
        PROJECT_SETTINGS
            .replacen(
                "\n}",
                ",\n  \"cached_toolpath\": \"Metadata/plate_1.gcode\"\n}",
                1,
            )
            .into_bytes(),
    );
    replace(
        &mut entries,
        "Metadata/model_settings.config",
        MODEL_SETTINGS
            .replacen(
                "</config>",
                "<metadata key=\"preview\" value=\"Metadata/top_1.png\"/></config>",
                1,
            )
            .into_bytes(),
    );

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    let codes = issue_codes(&report);

    assert!(codes.contains(&OutputIssueCode::ForbiddenSlicedArtifact));
    assert!(codes.contains(&OutputIssueCode::StaleDerivedArtifact));
}

#[test]
fn orca_output_name_templates_are_not_embedded_toolpath_references() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "Metadata/project_settings.config",
        PROJECT_SETTINGS
            .replacen(
                "\n}",
                ",\n  \"filename_format\": \"{input_filename_base}_{filament_type[0]}_{print_time}.gcode\"\n}",
                1,
            )
            .into_bytes(),
    );

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );

    assert!(report.is_valid, "unexpected issues: {:?}", report.issues);
}

#[test]
fn model_entry_uses_the_larger_limit_case_insensitively() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "_rels/.rels",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Target="/3D/Objects/ROOT.MODEL" Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
          <Relationship Target="/Auxiliaries/.thumbnails/thumbnail_3mf.png" Id="thumbnail" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
        </Relationships>"#
            .as_bytes()
            .to_vec(),
    );
    let model_entry = entries
        .iter_mut()
        .find(|(name, _)| name == "3D/3dmodel.model")
        .unwrap();
    model_entry.0 = "3D/Objects/ROOT.MODEL".into();
    model_entry.1 = MAIN_MODEL
        .replace("</model>", &format!("{}</model>", " ".repeat(1500)))
        .into_bytes();

    let mut policy = OutputValidationPolicy::strict_unsliced();
    policy.limits.max_entry_uncompressed_bytes = 1024;
    policy.limits.max_model_entry_uncompressed_bytes = 4096;
    policy.limits.max_xml_json_parse_bytes = 128;
    policy.limits.max_config_bytes = 4096;
    let report = validate_output_reader(Cursor::new(write_archive(&entries)), &policy);

    assert!(report.is_valid, "{:#?}", report.issues);
    assert!(
        OutputValidationPolicy::default()
            .limits
            .max_model_entry_uncompressed_bytes
            >= 619_366_496
    );
}

#[test]
fn validator_rejects_an_oversized_single_xml_token_before_publication() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "3D/3dmodel.model",
        MAIN_MODEL
            .replacen(
                "<resources>",
                &format!("<metadata>{}</metadata><resources>", "X".repeat(512)),
                1,
            )
            .into_bytes(),
    );
    let mut policy = OutputValidationPolicy::strict_unsliced();
    policy.limits.max_xml_token_bytes = 128;

    let report = validate_output_reader(Cursor::new(write_archive(&entries)), &policy);

    assert!(!report.is_valid);
    assert!(issue_codes(&report).contains(&OutputIssueCode::InvalidXml));
    assert!(report.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::InvalidXml
            && issue.message.contains("XML text token exceeds")
    }));
}

#[test]
fn production_extension_graph_closes_across_model_parts() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "3D/3dmodel.model",
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                 xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
                 requiredextensions="p">
          <resources>
            <object id="1" type="model" p:UUID="bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb">
              <components><component p:path="/3D/Objects/part.model" objectid="7" p:UUID="dddddddd-dddd-4ddd-8ddd-dddddddddddd"/></components>
            </object>
          </resources>
          <build p:UUID="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"><item objectid="1" p:UUID="cccccccc-cccc-4ccc-8ccc-cccccccccccc"/></build>
        </model>"#
            .as_bytes()
            .to_vec(),
    );
    entries.extend([
        (
            "3D/_rels/3dmodel.model.rels".into(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
              <Relationship Target="/3D/Objects/part.model" Id="part" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
            </Relationships>"#
                .as_bytes()
                .to_vec(),
        ),
        (
            "3D/Objects/part.model".into(),
            r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                     xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
                     requiredextensions="p">
              <resources><object id="7" type="model" p:UUID="eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"><mesh><vertices/><triangles/></mesh></object></resources>
              <build/>
            </model>"#
                .as_bytes()
                .to_vec(),
        ),
    ]);

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );

    assert!(report.is_valid, "{:#?}", report.issues);
}

#[test]
fn production_alias_and_direct_build_item_path_are_supported() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "3D/3dmodel.model",
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                 xmlns:prod="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
                 requiredextensions="prod">
          <resources/>
          <build prod:UUID="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa">
            <item prod:path="/3D/Objects/part.model" objectid="7" prod:UUID="bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"/>
          </build>
        </model>"#
            .as_bytes()
            .to_vec(),
    );
    entries.extend([
        (
            "3D/_rels/3dmodel.model.rels".into(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
              <Relationship Target="/3D/Objects/part.model" Id="part" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
            </Relationships>"#
                .as_bytes()
                .to_vec(),
        ),
        (
            "3D/Objects/part.model".into(),
            r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                     xmlns:prod="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
              <resources><object id="7" type="model" prod:UUID="cccccccc-cccc-4ccc-8ccc-cccccccccccc"><mesh><vertices/><triangles/></mesh></object></resources>
              <build/>
            </model>"#
                .as_bytes()
                .to_vec(),
        ),
    ]);

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );

    assert!(report.is_valid, "{:#?}", report.issues);

    let mut missing_required = entries;
    let root_model = String::from_utf8(
        missing_required
            .iter()
            .find(|(name, _)| name == "3D/3dmodel.model")
            .unwrap()
            .1
            .clone(),
    )
    .unwrap()
    .replace(" requiredextensions=\"prod\"", "");
    replace(
        &mut missing_required,
        "3D/3dmodel.model",
        root_model.into_bytes(),
    );
    let missing_required_report = validate_output_reader(
        Cursor::new(write_archive(&missing_required)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(
        issue_codes(&missing_required_report).contains(&OutputIssueCode::InvalidModelNamespace)
    );
}

#[test]
fn root_level_model_relationship_and_uuid_only_production_are_supported() {
    let mut entries = valid_entries()
        .into_iter()
        .map(|(name, bytes)| {
            if name == "3D/3dmodel.model" {
                ("root.model".to_owned(), bytes)
            } else {
                (name, bytes)
            }
        })
        .collect::<Vec<_>>();
    replace(
        &mut entries,
        "_rels/.rels",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Target="/root.model" Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
          <Relationship Target="/Auxiliaries/.thumbnails/thumbnail_3mf.png" Id="thumbnail" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
        </Relationships>"#
            .as_bytes()
            .to_vec(),
    );
    replace(
        &mut entries,
        "root.model",
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                 xmlns:production="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
          <resources><object id="1" type="model" production:UUID="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"><mesh><vertices/><triangles/></mesh></object></resources>
          <build production:UUID="bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"><item objectid="1" production:UUID="cccccccc-cccc-4ccc-8ccc-cccccccccccc"/></build>
        </model>"#
            .as_bytes()
            .to_vec(),
    );
    entries.push((
        "_rels/root.model.rels".into(),
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#
            .as_bytes()
            .to_vec(),
    ));

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );

    assert!(report.is_valid, "{:#?}", report.issues);
}

#[test]
fn duplicate_expanded_production_uuid_attributes_are_rejected_across_aliases() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "3D/3dmodel.model",
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                 xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
                 xmlns:q="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
          <resources>
            <object id="1" type="model"
                    p:UUID="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
                    q:UUID="dddddddd-dddd-4ddd-8ddd-dddddddddddd">
              <mesh><vertices/><triangles/></mesh>
            </object>
          </resources>
          <build p:UUID="bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb">
            <item objectid="1" p:UUID="cccccccc-cccc-4ccc-8ccc-cccccccccccc"/>
          </build>
        </model>"#
            .as_bytes()
            .to_vec(),
    );

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );

    assert!(!report.is_valid);
    assert!(report.issues.iter().any(|issue| {
        issue.code == OutputIssueCode::InvalidProductionUuid
            && issue
                .message
                .contains("more than once through namespace aliases")
    }));
}

#[test]
fn production_relationships_must_be_used_and_cannot_self_reference() {
    let mut unused_entries = valid_entries();
    unused_entries.extend([
        (
            "3D/_rels/3dmodel.model.rels".into(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
              <Relationship Target="/3D/Objects/unused.model" Id="unused" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
            </Relationships>"#
                .as_bytes()
                .to_vec(),
        ),
        (
            "3D/Objects/unused.model".into(),
            MAIN_MODEL.as_bytes().to_vec(),
        ),
    ]);
    let unused_report = validate_output_reader(
        Cursor::new(write_archive(&unused_entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(
        issue_codes(&unused_report).contains(&OutputIssueCode::UnusedProductionModelRelationship)
    );

    let mut self_entries = valid_entries();
    self_entries.push((
        "3D/_rels/3dmodel.model.rels".into(),
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
          <Relationship Target="/3D/3dmodel.model" Id="self" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
        </Relationships>"#
            .as_bytes()
            .to_vec(),
    ));
    let self_report = validate_output_reader(
        Cursor::new(write_archive(&self_entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    assert!(issue_codes(&self_report).contains(&OutputIssueCode::ProductionSelfReference));
}

#[test]
fn production_extension_graph_reports_uuid_object_path_and_dtd_failures() {
    let mut entries = valid_entries();
    replace(
        &mut entries,
        "3D/3dmodel.model",
        r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                 xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" requiredextensions="p"
                 p:UUID="not-a-uuid">
          <resources>
            <object id="1" type="model" p:UUID="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa">
              <components>
                <component objectid="42"/>
                <component p:path="/3D/Objects/part.model" objectid="999"/>
              </components>
            </object>
            <object id="1" type="model" p:UUID="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"/>
          </resources>
          <build><item objectid="77"/></build>
        </model>"#
            .as_bytes()
            .to_vec(),
    );
    entries.extend([
        (
            "3D/Objects/part.model".into(),
            r#"<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
                     xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
                     requiredextensions="p">
              <resources>
                <object id="7" type="model" p:UUID="AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA">
                  <components><component p:path="/3D/3dmodel.model" objectid="1"/></components>
                </object>
                <object id="2147483648" type="model"/>
              </resources>
              <build/>
            </model>"#
                .as_bytes()
                .to_vec(),
        ),
        (
            "Metadata/doctype.xml".into(),
            b"<!DOCTYPE config [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><config/>".to_vec(),
        ),
    ]);

    let report = validate_output_reader(
        Cursor::new(write_archive(&entries)),
        &OutputValidationPolicy::strict_unsliced(),
    );
    let codes = issue_codes(&report);

    assert!(!report.is_valid);
    assert!(codes.contains(&OutputIssueCode::DuplicateObjectId));
    assert!(codes.contains(&OutputIssueCode::InvalidProductionUuid));
    assert!(codes.contains(&OutputIssueCode::DuplicateProductionUuid));
    assert!(codes.contains(&OutputIssueCode::MissingObjectReference));
    assert!(codes.contains(&OutputIssueCode::MissingProductionPathRelationship));
    assert!(codes.contains(&OutputIssueCode::ProductionPathOutsideRootModel));
    assert!(codes.contains(&OutputIssueCode::OrphanModelPart));
    assert!(codes.contains(&OutputIssueCode::InvalidModelReference));
    assert!(codes.contains(&OutputIssueCode::ForbiddenXmlDoctype));
}

#[test]
fn archive_and_total_size_limits_stop_before_inflation() {
    let bytes = write_archive(&valid_entries());

    let mut archive_policy = OutputValidationPolicy::strict_unsliced();
    archive_policy.limits.max_archive_bytes = 1;
    let archive_report = validate_output_reader(Cursor::new(bytes.clone()), &archive_policy);
    assert!(!archive_report.is_valid);
    assert_eq!(archive_report.statistics.crc_checked_entry_count, 0);
    assert!(issue_codes(&archive_report).contains(&OutputIssueCode::ArchiveTooLarge));

    let mut total_policy = OutputValidationPolicy::strict_unsliced();
    total_policy.limits.max_total_uncompressed_bytes = 1;
    let total_report = validate_output_reader(Cursor::new(bytes), &total_policy);
    assert!(!total_report.is_valid);
    assert_eq!(total_report.statistics.crc_checked_entry_count, 0);
    assert!(issue_codes(&total_report).contains(&OutputIssueCode::ArchiveTooLarge));
}

#[test]
fn advertised_entry_limit_is_checked_before_zip_archive_allocation() {
    let mut bytes = write_archive(&valid_entries());
    let eocd = bytes
        .windows(4)
        .rposition(|window| window == [0x50, 0x4b, 0x05, 0x06])
        .unwrap();
    bytes[eocd + 8..eocd + 10].copy_from_slice(&101_u16.to_le_bytes());
    bytes[eocd + 10..eocd + 12].copy_from_slice(&101_u16.to_le_bytes());
    let mut policy = OutputValidationPolicy::strict_unsliced();
    policy.limits.max_entries = 100;

    let report = validate_output_reader(Cursor::new(bytes), &policy);

    assert!(!report.is_valid);
    assert_eq!(report.statistics.entry_count, 101);
    assert!(issue_codes(&report).contains(&OutputIssueCode::TooManyEntries));
    assert_eq!(report.statistics.crc_checked_entry_count, 0);
}
