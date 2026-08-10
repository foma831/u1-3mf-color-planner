use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use u1_three_mf::{
    AnalysisLimits, ProjectDialect, SourceApplication, WarningCode, analyze_project,
};

fn sample_path() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        PathBuf::from("../../../Sample/Withered_Foxy.3mf"),
        manifest.join("../../../Sample/Withered_Foxy.3mf"),
        manifest.join("../../Sample/Withered_Foxy.3mf"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .expect("Sample/Withered_Foxy.3mf must be present")
}

fn named_sample_path(file_name: &str) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        PathBuf::from("../../../Sample").join(file_name),
        manifest.join("../../../Sample").join(file_name),
        manifest.join("../../Sample").join(file_name),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .unwrap_or_else(|| panic!("Sample/{file_name} must be present"))
}

fn file_sha256(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    format!("{:x}", digest.finalize())
}

#[test]
fn withered_foxy_acceptance_counts_and_fixture_identity() {
    let path = sample_path();
    let expected_hash = "f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81";
    assert_eq!(file_sha256(&path), expected_hash);

    let analysis = analyze_project(&path).expect("sample should analyze successfully");
    assert_eq!(analysis.input.sha256, expected_hash);
    assert_eq!(analysis.source.application, SourceApplication::BambuStudio);
    assert_eq!(analysis.source.dialect, ProjectDialect::BambuStudioProject);
    assert_eq!(analysis.summary.plate_count, 12);
    assert_eq!(analysis.summary.object_count, 89);
    assert_eq!(analysis.summary.part_count, 390);
    assert_eq!(analysis.summary.declared_filament_count, 11);
    // Facet paint state 7 (encoded as `4C`) is present on printable Head/Jaw
    // parts, so correct TriangleSelector decoding reports 10 effective slots.
    assert_eq!(analysis.summary.used_filament_count, 10);
    assert_eq!(analysis.summary.mono_object_count, 74);
    assert_eq!(analysis.summary.multi_color_object_count, 15);
    assert_eq!(analysis.summary.bounded_printable_part_count, 388);
    assert_eq!(analysis.summary.bounded_object_count, 89);
    assert_eq!(analysis.summary.bounded_instance_count, 89);
    let head = analysis
        .objects
        .iter()
        .find(|object| object.name.as_deref() == Some("Head"))
        .expect("Head object");
    assert_eq!(head.effective_slots, [1, 2, 3, 4, 5, 7, 8]);
    assert_eq!(head.effective_material_colors.len(), 7);
    assert!(analysis.plates.iter().all(|plate| {
        plate
            .printable_bounds
            .is_some_and(|bounds| bounds.is_valid())
    }));
    let hinge_plate = analysis
        .plates
        .iter()
        .find(|plate| plate.id == 11)
        .expect("hinge plate");
    let hinge_size = hinge_plate.printable_bounds.unwrap().size().unwrap();
    assert!(hinge_size[0] > 180.0);
    assert_eq!(analysis.warnings.len(), 1);
    assert_eq!(
        analysis.warnings[0].code,
        WarningCode::PossibleAlternativePlates
    );
    assert_eq!(file_sha256(&path), expected_hash);
}

#[test]
fn a1_mini_no_ams_sample_identity_and_target_are_stable() {
    let path = named_sample_path("Withered_Foxy_A1_mini_No_AMS.3mf");
    let expected_hash = "78a613193c05f96c77e0db5c7a5a0ce0eb051936752ae2b0def09ae8202cab8c";
    assert_eq!(file_sha256(&path), expected_hash);

    let analysis = analyze_project(&path).expect("A1 mini sample should analyze successfully");
    assert_eq!(analysis.input.sha256, expected_hash);
    assert_eq!(analysis.printer.model.as_deref(), Some("Bambu Lab A1 mini"));
    assert_eq!(analysis.printer.nozzle_diameters_mm, [0.4]);
    assert_eq!(analysis.summary.plate_count, 9);
    assert_eq!(analysis.summary.object_count, 89);
    assert_eq!(file_sha256(&path), expected_hash);
}

#[test]
#[ignore = "large optional fixture; run explicitly when qualifying analysis limits"]
fn sailfin_u1_monolithic_model_exceeds_generic_limit_but_analyzes_by_default() {
    let path = named_sample_path("Sailfin Dragon - Articulated Lizard by Raki-Box.3mf");
    let expected_hash = "1b20d6124353d3bc31c4ea554e482ba6f8ef281dd23e2df6bbc0d561b4f6e0d7";
    assert_eq!(file_sha256(&path), expected_hash);

    let analysis = analyze_project(&path).expect("large U1 model XML should analyze successfully");
    let limits = AnalysisLimits::default();
    assert_eq!(analysis.input.sha256, expected_hash);
    assert!(
        analysis.archive.largest_entry_uncompressed_bytes > limits.max_entry_uncompressed_bytes
    );
    assert!(
        analysis.archive.largest_entry_uncompressed_bytes
            < limits.max_model_entry_uncompressed_bytes
    );
    assert_eq!(analysis.printer.model.as_deref(), Some("Snapmaker U1"));
    assert_eq!(analysis.printer.nozzle_diameters_mm, [0.4; 4]);
    assert_eq!(analysis.summary.plate_count, 1);
    assert_eq!(analysis.summary.object_count, 4);
    assert_eq!(analysis.summary.vertex_count, 3_428_075);
    assert_eq!(analysis.summary.triangle_count, 6_855_906);
    assert_eq!(file_sha256(&path), expected_hash);
}
