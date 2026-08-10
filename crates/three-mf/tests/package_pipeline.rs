use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::tempdir;
use u1_three_mf::{
    ContentTypesBuilder, ExpectedSourceIdentity, InputIdentity, MAIN_MODEL_PATH,
    MODEL_RELATIONSHIP_TYPE, OpcPackageWriter, OpcRelationship, OpcWriteError,
    OutputValidationPolicy, PackageBuildManifest, ProductionBuildItem, ProductionComponent,
    ProductionModel, ProductionObject, StagedPackageValidationError, StructuralOutputValidator,
    analyze_project, relationships_xml,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const PART_MODEL_PATH: &str = "3D/Objects/part.model";

const PART_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
       unit="millimeter" requiredextensions="p">
 <resources>
  <object id="7" type="model" p:UUID="00000000-0000-4000-8000-000000000005">
   <mesh>
    <vertices>
     <vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/>
     <vertex x="0" y="10" z="0"/>
    </vertices>
    <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
   </mesh>
  </object>
 </resources>
 <build/>
</model>"#;

const PROJECT_SETTINGS: &str = r##"{
  "printer_model": "Stage A Generic Printer",
  "printer_settings_id": "Stage A Generic Printer 0.4 nozzle",
  "print_settings_id": "Stage A Unsliced Process",
  "nozzle_diameter": ["0.4"],
  "filament_settings_id": ["Generic PLA"],
  "filament_colour": ["#808080"],
  "filament_type": ["PLA"]
}"##;

const MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
 <object id="1"><metadata key="name" value="Stage A fixture"/></object>
 <plate>
  <metadata key="plater_id" value="1"/>
  <metadata key="plater_name" value="Fixture"/>
  <model_instance>
   <metadata key="object_id" value="1"/>
   <metadata key="instance_id" value="0"/>
  </model_instance>
 </plate>
</config>"#;

fn package_writer(part_source: Option<(&Path, ExpectedSourceIdentity)>) -> OpcPackageWriter {
    let mut content_types = ContentTypesBuilder::project_3mf();
    content_types
        .add_override("/Metadata/project_settings.config", "application/json")
        .unwrap();
    content_types
        .add_override("/Metadata/model_settings.config", "application/xml")
        .unwrap();

    let mut model = ProductionModel::new("00000000-0000-4000-8000-000000000001");
    model.metadata = BTreeMap::from([
        (
            "Application".to_owned(),
            "U1 3MF Color Planner Stage A".to_owned(),
        ),
        ("Title".to_owned(), "Stage A package fixture".to_owned()),
    ]);
    model.objects.push(ProductionObject {
        id: 1,
        uuid: "00000000-0000-4000-8000-000000000002".to_owned(),
        name: Some("Stage A fixture".to_owned()),
        components: vec![ProductionComponent {
            path: format!("/{PART_MODEL_PATH}"),
            object_id: 7,
            uuid: "00000000-0000-4000-8000-000000000003".to_owned(),
            transform: None,
        }],
    });
    model.build_items.push(ProductionBuildItem {
        object_id: 1,
        uuid: "00000000-0000-4000-8000-000000000004".to_owned(),
        transform: None,
        printable: true,
    });

    let root_relationship =
        OpcRelationship::internal("rel-1", MODEL_RELATIONSHIP_TYPE, MAIN_MODEL_PATH).unwrap();
    let model_relationships = model.relationship_entries().unwrap();

    let mut writer = OpcPackageWriter::new();
    writer
        .add_bytes("[Content_Types].xml", content_types.to_xml().unwrap())
        .unwrap()
        .add_bytes(
            "_rels/.rels",
            relationships_xml(&[root_relationship]).unwrap(),
        )
        .unwrap()
        .add_bytes(MAIN_MODEL_PATH, model.to_xml().unwrap())
        .unwrap()
        .add_bytes(
            "3D/_rels/3dmodel.model.rels",
            relationships_xml(&model_relationships).unwrap(),
        )
        .unwrap();
    if let Some((source_path, expected_identity)) = part_source {
        writer
            .copy_zip_entry_raw(source_path, expected_identity, PART_MODEL_PATH)
            .unwrap();
    } else {
        writer
            .add_bytes(PART_MODEL_PATH, PART_MODEL.as_bytes())
            .unwrap();
    }
    writer
        .add_bytes(
            "Metadata/project_settings.config",
            PROJECT_SETTINGS.as_bytes(),
        )
        .unwrap()
        .add_bytes("Metadata/model_settings.config", MODEL_SETTINGS.as_bytes())
        .unwrap();
    writer
}

fn source_archive(directory: &Path) -> (PathBuf, InputIdentity, ExpectedSourceIdentity) {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        PART_MODEL_PATH,
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )
    .unwrap();
    zip.write_all(PART_MODEL.as_bytes()).unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    let path = directory.join("analyzed-source.3mf");
    fs::write(&path, &bytes).unwrap();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let source = InputIdentity {
        byte_size: bytes.len() as u64,
        sha256: sha256.clone(),
    };
    let expected = ExpectedSourceIdentity::new(source.byte_size, sha256).unwrap();
    (path, source, expected)
}

#[test]
fn package_is_deterministic_validated_before_publish_and_analyzable_after_publish() {
    let directory = tempdir().unwrap();
    let first_destination = directory.path().join("first.3mf");
    let second_destination = directory.path().join("second.3mf");
    let (source_path, source_identity, expected_source) = source_archive(directory.path());
    let writer = package_writer(Some((&source_path, expected_source)));
    let validator = StructuralOutputValidator::new(OutputValidationPolicy::strict_unsliced());

    let first = writer.stage_to(&first_destination).unwrap();
    assert!(!first_destination.exists());
    let first_validation = validator.validate_path(first.path()).unwrap();
    first_validation.ensure_publishable().unwrap();

    let second = writer.stage_to(&second_destination).unwrap();
    let second_validation = validator.validate_path(second.path()).unwrap();
    second_validation.ensure_publishable().unwrap();
    assert_eq!(
        first.report().package_sha256,
        second.report().package_sha256
    );
    assert_eq!(first_validation, second_validation);

    let first = first.validate().unwrap();
    let second = second.validate().unwrap();
    assert_eq!(first.validation_report(), &first_validation);
    assert_eq!(second.validation_report(), &second_validation);
    let first_manifest =
        PackageBuildManifest::from_validated_package(&source_identity, &first, Vec::new()).unwrap();
    let second_manifest =
        PackageBuildManifest::from_validated_package(&source_identity, &second, Vec::new())
            .unwrap();
    assert_eq!(
        first_manifest.to_canonical_json().unwrap(),
        second_manifest.to_canonical_json().unwrap()
    );

    first.publish().unwrap();
    assert!(first_destination.exists());
    assert!(!second_destination.exists());
    drop(second);

    let duplicate = writer
        .stage_to(&first_destination)
        .unwrap()
        .validate()
        .unwrap();
    assert!(matches!(
        duplicate.publish(),
        Err(OpcWriteError::OutputExists(_))
    ));

    let analysis = analyze_project(&first_destination).unwrap();
    assert_eq!(analysis.summary.plate_count, 1);
    assert_eq!(analysis.summary.object_count, 1);
    assert_eq!(analysis.summary.instance_count, 1);
    assert_eq!(analysis.summary.triangle_count, 1);
    assert!(!analysis.source.has_sliced_artifacts);
}

#[test]
fn analyzer_reports_slice_metadata_and_previews_as_sliced_artifacts() {
    let directory = tempdir().unwrap();
    let destination = directory.path().join("gui-saved.3mf");
    let mut writer = package_writer(None);
    writer
        .add_bytes("Metadata/slice_info.config", b"<config/>")
        .unwrap()
        .add_bytes("Metadata/plate_1.json", b"{}")
        .unwrap()
        .add_bytes("Metadata/top_1.png", b"preview")
        .unwrap();
    let staged = writer.stage_to(&destination).unwrap();

    let analysis = analyze_project(staged.path()).unwrap();

    assert!(analysis.source.has_sliced_artifacts);
    assert_eq!(
        analysis.source.sliced_artifact_entries,
        vec![
            "Metadata/plate_1.json",
            "Metadata/slice_info.config",
            "Metadata/top_1.png",
        ]
    );
}

#[test]
fn validation_cannot_be_borrowed_from_other_bytes_then_restored_for_publication() {
    let directory = tempdir().unwrap();
    let destination = directory.path().join("invalid.3mf");
    let valid_destination = directory.path().join("valid.3mf");
    let mut invalid_writer = OpcPackageWriter::new();
    invalid_writer
        .add_bytes("not-a-project", b"original payload")
        .unwrap();
    let staged = invalid_writer.stage_to(&destination).unwrap();
    let valid_stage = package_writer(None).stage_to(&valid_destination).unwrap();

    // This is the old capability attack: substitute structurally valid bytes
    // for validation, then restore the report-bound invalid bytes before
    // publication. Validation now first snapshots only bytes matching the
    // write report into an unexposed private directory, so the substitution is
    // rejected before a publication capability can exist.
    fs::write(staged.path(), fs::read(valid_stage.path()).unwrap()).unwrap();
    let error = staged.validate().unwrap_err();

    assert!(matches!(
        error,
        StagedPackageValidationError::Prepare(OpcWriteError::StagedPackageIdentityMismatch { .. })
    ));
    assert!(!destination.exists());
}

#[test]
fn structurally_invalid_stage_cannot_reach_the_publication_type() {
    let directory = tempdir().unwrap();
    let destination = directory.path().join("invalid.3mf");
    let mut writer = OpcPackageWriter::new();
    writer.add_bytes("not-a-project", b"payload").unwrap();
    let staged = writer.stage_to(&destination).unwrap();

    let error = staged.validate().unwrap_err();
    assert!(error.report().is_some_and(|report| !report.is_valid));
    assert!(!destination.exists());
}
