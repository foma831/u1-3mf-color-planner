use std::io::Write;

use tempfile::NamedTempFile;
use u1_three_mf::{
    AnalysisError, AnalysisLimits, ColorClassification, ProjectDialect, SourceApplication,
    VolumeType, WarningCode, analyze_project, analyze_project_with_limits,
};
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

const MAIN_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/Objects/parts.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

const MAIN_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
       xmlns:BambuStudio="http://schemas.bambulab.com/package/2021"
       unit="millimeter" requiredextensions="p">
  <metadata name="Application">BambuStudio-02.06.00.51</metadata>
  <metadata name="BambuStudio:3mfVersion">1</metadata>
  <metadata name="Title">Synthetic project</metadata>
  <resources>
    <object id="10" type="model"><components>
      <component p:path="/3D/Objects/parts.model" objectid="100" transform="2 0 0 0 3 0 0 0 0.5 1 2 3"/>
    </components></object>
    <object id="20" type="model"><components>
      <component p:path="/3D/Objects/parts.model" objectid="200"/>
      <component p:path="/3D/Objects/parts.model" objectid="201"/>
    </components></object>
  </resources>
  <build>
    <item objectid="10" transform="1 0 0 0 1 0 0 0 1 100 200 300" printable="1"/>
    <item objectid="20" printable="1"/>
    <item objectid="10" transform="0 1 0 0 0 1 1 0 0 5 6 7" printable="1"/>
  </build>
</model>"#;

const PRODUCTION_BUILD_PATH_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:production="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
       unit="millimeter" requiredextensions="production">
  <resources/>
  <build>
    <item production:path="/3D/Objects/parts.model" objectid="200"
          transform="2 0 0 0 3 0 0 0 4 10 20 30" printable="1"/>
  </build>
</model>"#;

const PARTS_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
       unit="millimeter" requiredextensions="p">
  <resources>
    <object id="100" type="model"><components>
      <component p:path="nested.model" objectid="101" transform="0 1 0 -1 0 0 0 0 1 10 20 30"/>
    </components></object>
    <object id="200" type="model"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
    </mesh></object>
    <object id="201" type="model"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="5" y="0" z="0"/><vertex x="0" y="5" z="0"/><vertex x="0" y="0" z="5"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
    </mesh></object>
  </resources><build/>
</model>"#;

const PARTS_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="nested.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

const NESTED_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="centimeter">
  <resources>
    <object id="101" type="model"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="0.2" y="0" z="0"/><vertex x="0" y="0.3" z="0"/><vertex x="0" y="0" z="0.4"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2" paint_color="8"/></triangles>
    </mesh></object>
  </resources><build/>
</model>"#;

const PROJECT_SETTINGS: &str = r##"{
  "version": "02.06.00.51",
  "printer_model": "Bambu Lab P1S",
  "printer_variant": "0.4",
  "nozzle_diameter": ["0.4"],
  "print_settings_id": "0.12mm Test",
  "layer_height": "0.12",
  "initial_layer_print_height": "0.2",
  "enable_prime_tower": "1",
  "filament_colour": ["#AA0000", "#00AA00", "#0000AA"],
  "filament_type": ["PLA", "PLA", "PETG"],
  "filament_settings_id": ["Red PLA", "Green PLA", "Blue PETG"]
}"##;

const MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="10">
    <metadata key="name" value="Painted object"/>
    <metadata key="extruder" value="1"/>
    <part id="100" subtype="normal_part"><metadata key="name" value="Body"/></part>
  </object>
  <object id="20">
    <metadata key="name" value="Negative-only object"/>
    <metadata key="extruder" value="3"/>
    <part id="200" subtype="negative_part"><metadata key="name" value="Cutout"/></part>
    <part id="201" subtype="modifier"><metadata key="name" value="Settings region"/></part>
  </object>
  <plate>
    <metadata key="plater_id" value="1"/><metadata key="plater_name" value="First"/>
    <model_instance><metadata key="object_id" value="10"/><metadata key="instance_id" value="0"/></model_instance>
  </plate>
  <plate>
    <metadata key="plater_id" value="2"/><metadata key="plater_name" value="Copies"/>
    <model_instance><metadata key="object_id" value="10"/><metadata key="instance_id" value="1"/></model_instance>
    <model_instance><metadata key="object_id" value="20"/><metadata key="instance_id" value="0"/></model_instance>
  </plate>
</config>"#;

fn valid_entries() -> Vec<(&'static str, Vec<u8>, CompressionMethod)> {
    vec![
        (
            CONTENT_TYPES_PATH,
            CONTENT_TYPES.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "_rels/.rels",
            ROOT_RELS.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/3dmodel.model",
            MAIN_MODEL.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/_rels/3dmodel.model.rels",
            MAIN_RELS.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/Objects/parts.model",
            PARTS_MODEL.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/Objects/_rels/parts.model.rels",
            PARTS_RELS.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/Objects/nested.model",
            NESTED_MODEL.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "Metadata/project_settings.config",
            PROJECT_SETTINGS.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "Metadata/model_settings.config",
            MODEL_SETTINGS.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
    ]
}

fn production_build_path_entries() -> Vec<(&'static str, Vec<u8>, CompressionMethod)> {
    valid_entries()
        .into_iter()
        .filter(|(name, _, _)| {
            !matches!(
                *name,
                "Metadata/project_settings.config" | "Metadata/model_settings.config"
            )
        })
        .map(|(name, data, method)| {
            if name == "3D/3dmodel.model" {
                (
                    name,
                    PRODUCTION_BUILD_PATH_MODEL.as_bytes().to_vec(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect()
}

const CONTENT_TYPES_PATH: &str = "[Content_Types].xml";

fn write_archive(entries: Vec<(&str, Vec<u8>, CompressionMethod)>) -> NamedTempFile {
    let file = NamedTempFile::new().unwrap();
    let mut writer = ZipWriter::new(file.reopen().unwrap());
    for (name, data, method) in entries {
        writer
            .start_file(
                name,
                SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    writer.finish().unwrap();
    file
}

#[test]
fn analyzes_bambu_metadata_instances_volume_types_and_paint() {
    let archive = write_archive(valid_entries());
    let analysis = analyze_project(archive.path()).unwrap();

    assert_eq!(analysis.source.application, SourceApplication::BambuStudio);
    assert_eq!(analysis.source.dialect, ProjectDialect::BambuStudioProject);
    assert_eq!(analysis.source.title.as_deref(), Some("Synthetic project"));
    assert_eq!(analysis.printer.model.as_deref(), Some("Bambu Lab P1S"));
    assert_eq!(analysis.process.layer_height_mm, Some(0.12));
    assert_eq!(analysis.summary.object_count, 2);
    assert_eq!(analysis.summary.instance_count, 3);
    assert_eq!(analysis.summary.plate_count, 2);
    assert_eq!(analysis.summary.part_count, 3);
    assert_eq!(analysis.summary.declared_filament_count, 3);
    assert_eq!(analysis.summary.used_filament_count, 2);

    let painted = analysis
        .objects
        .iter()
        .find(|object| object.id == 10)
        .unwrap();
    assert_eq!(painted.instance_count, 2);
    assert_eq!(painted.plate_ids, [1, 2]);
    assert_eq!(painted.effective_slots, [1, 2]);
    assert_eq!(painted.classification, ColorClassification::MultiColor);
    assert_eq!(painted.parts[0].painted_slots, [2]);
    assert_eq!(painted.parts[0].component_transform.unwrap().values[0], 2.0);
    let part_bounds = painted.parts[0].object_space_bounds.unwrap();
    assert_eq!(part_bounds.min, [15.0, 62.0, 18.0]);
    assert_eq!(part_bounds.max, [21.0, 68.0, 20.0]);
    assert_eq!(painted.printable_bounds, Some(part_bounds));

    let first_instance = &analysis.plates[0].instances[0];
    assert_eq!(
        first_instance.printable_bounds.unwrap().min,
        [115.0, 262.0, 318.0]
    );
    assert_eq!(
        first_instance.printable_bounds.unwrap().max,
        [121.0, 268.0, 320.0]
    );
    let second_instance = &analysis.plates[1].instances[0];
    assert_eq!(
        second_instance.printable_bounds.unwrap().min,
        [23.0, 21.0, 69.0]
    );
    assert_eq!(
        second_instance.printable_bounds.unwrap().max,
        [25.0, 27.0, 75.0]
    );

    let negative = analysis
        .objects
        .iter()
        .find(|object| object.id == 20)
        .unwrap();
    assert_eq!(negative.parts[0].volume_type, VolumeType::NegativePart);
    assert!(!negative.parts[0].printable);
    assert!(negative.effective_slots.is_empty());
    assert_eq!(negative.classification, ColorClassification::Unassigned);
    assert!(negative.parts[0].object_space_bounds.is_some());
    assert_eq!(negative.parts[1].volume_type, VolumeType::Modifier);
    assert!(!negative.parts[1].printable);
    assert!(negative.parts[1].object_space_bounds.is_some());
    assert!(negative.printable_bounds.is_none());
    assert!(!analysis.plates[1].instances[1].printable);
    assert!(analysis.plates[1].instances[1].printable_bounds.is_none());
    assert_eq!(analysis.summary.bounded_printable_part_count, 1);
    assert_eq!(analysis.summary.bounded_object_count, 1);
    assert_eq!(analysis.summary.bounded_instance_count, 2);

    serde_json::to_value(&analysis).expect("analysis should serialize to JSON");
}

#[test]
fn mesh_fast_paths_still_reject_duplicate_vertex_and_triangle_attributes() {
    for malformed_model in [
        NESTED_MODEL.replace(
            r#"<vertex x="0" y="0" z="0"/>"#,
            r#"<vertex x="0" x="1" y="0" z="0"/>"#,
        ),
        NESTED_MODEL.replace(
            r#"<triangle v1="0" v2="1" v3="2" paint_color="8"/>"#,
            r#"<triangle v1="0" v2="1" v3="2" v1="1" paint_color="8"/>"#,
        ),
    ] {
        let entries = valid_entries()
            .into_iter()
            .map(|(name, data, method)| {
                if name == "3D/Objects/nested.model" {
                    (name, malformed_model.as_bytes().to_vec(), method)
                } else {
                    (name, data, method)
                }
            })
            .collect();
        assert!(matches!(
            analyze_project(write_archive(entries).path()),
            Err(AnalysisError::InvalidXml { message, .. })
                if message.contains("duplicated attribute")
        ));
    }
}

#[test]
fn duplicate_root_object_ids_with_conflicting_mesh_and_paint_fail_closed() {
    const DUPLICATE_ROOT_OBJECT_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:BambuStudio="http://schemas.bambulab.com/package/2021"
       unit="millimeter">
  <metadata name="Application">BambuStudio-02.06.00.51</metadata>
  <resources>
    <object id="10" type="model"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2" paint_color="8"/></triangles>
    </mesh></object>
    <object id="10" type="model"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="0" y="20" z="0"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2" paint_color="16"/></triangles>
    </mesh></object>
  </resources>
  <build><item objectid="10" printable="1"/></build>
</model>"#;

    let entries = valid_entries()
        .into_iter()
        .filter(|(name, _, _)| {
            !matches!(
                *name,
                "3D/_rels/3dmodel.model.rels"
                    | "3D/Objects/parts.model"
                    | "3D/Objects/_rels/parts.model.rels"
                    | "3D/Objects/nested.model"
                    | "Metadata/project_settings.config"
                    | "Metadata/model_settings.config"
            )
        })
        .map(|(name, data, method)| {
            if name == "3D/3dmodel.model" {
                (
                    name,
                    DUPLICATE_ROOT_OBJECT_MODEL.as_bytes().to_vec(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect();

    assert!(matches!(
        analyze_project(write_archive(entries).path()),
        Err(AnalysisError::InvalidStructure(message))
            if message == "duplicate object ID 10 in 3D/3dmodel.model"
    ));
}

#[test]
fn production_build_item_path_loads_external_geometry_and_applies_transform() {
    let archive = write_archive(production_build_path_entries());
    let analysis = analyze_project(archive.path()).unwrap();

    assert_eq!(analysis.summary.object_count, 1);
    assert_eq!(analysis.summary.instance_count, 1);
    assert_eq!(analysis.summary.plate_count, 1);

    let object = &analysis.objects[0];
    assert_eq!(object.id, 200);
    let object_bounds = object.printable_bounds.unwrap();
    assert_eq!(object_bounds.min, [0.0, 0.0, 0.0]);
    assert_eq!(object_bounds.max, [1.0, 1.0, 1.0]);

    let instance = &analysis.plates[0].instances[0];
    assert!(instance.printable);
    assert_eq!(
        instance.transform.unwrap().values,
        [
            2.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 4.0, 10.0, 20.0, 30.0
        ]
    );
    let instance_bounds = instance.printable_bounds.unwrap();
    assert_eq!(instance_bounds.min, [10.0, 20.0, 30.0]);
    assert_eq!(instance_bounds.max, [12.0, 23.0, 34.0]);
    assert!(!analysis.warnings.iter().any(|warning| {
        warning.code == WarningCode::DanglingObjectReference && warning.object_id == Some(200)
    }));
}

#[test]
fn production_build_paths_scope_duplicate_resource_ids_by_model_part() {
    const ROOT_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
       xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
       unit="millimeter" requiredextensions="p">
  <resources/>
  <build>
    <item p:path="/3D/Objects/alpha.model" objectid="1" printable="1"/>
    <item p:path="/3D/Objects/beta.model" objectid="1"
          transform="1 0 0 0 1 0 0 0 1 10 20 30" printable="1"/>
  </build>
</model>"#;
    const RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/Objects/alpha.model" Id="rel-alpha" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
  <Relationship Target="/3D/Objects/beta.model" Id="rel-beta" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;
    const ALPHA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
  <resources><object id="1" type="model"><mesh>
    <vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/><vertex x="0" y="0" z="1"/></vertices>
    <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
  </mesh></object></resources><build/>
</model>"#;
    const BETA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
  <resources><object id="1" type="model"><mesh>
    <vertices><vertex x="0" y="0" z="0"/><vertex x="2" y="0" z="0"/><vertex x="0" y="2" z="0"/><vertex x="0" y="0" z="2"/></vertices>
    <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
  </mesh></object></resources><build/>
</model>"#;

    let archive = write_archive(vec![
        (
            CONTENT_TYPES_PATH,
            CONTENT_TYPES.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "_rels/.rels",
            ROOT_RELS.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/3dmodel.model",
            ROOT_MODEL.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/_rels/3dmodel.model.rels",
            RELATIONSHIPS.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/Objects/alpha.model",
            ALPHA.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
        (
            "3D/Objects/beta.model",
            BETA.as_bytes().to_vec(),
            CompressionMethod::Stored,
        ),
    ]);
    let analysis = analyze_project(archive.path()).unwrap();

    assert_eq!(analysis.summary.object_count, 2);
    assert_eq!(analysis.summary.instance_count, 2);
    assert_ne!(analysis.objects[0].id, analysis.objects[1].id);

    let alpha = analysis
        .objects
        .iter()
        .find(|object| object.source_model_path.as_deref() == Some("3D/Objects/alpha.model"))
        .unwrap();
    assert_eq!(alpha.id, 1);
    assert_eq!(alpha.source_object_id, None);
    assert_eq!(alpha.printable_bounds.unwrap().max, [1.0, 1.0, 1.0]);

    let beta = analysis
        .objects
        .iter()
        .find(|object| object.source_model_path.as_deref() == Some("3D/Objects/beta.model"))
        .unwrap();
    assert_eq!(beta.source_object_id, Some(1));
    assert_eq!(beta.printable_bounds.unwrap().max, [2.0, 2.0, 2.0]);

    assert_eq!(analysis.plates[0].instances[0].object_id, alpha.id);
    assert_eq!(
        analysis.plates[0].instances[0]
            .printable_bounds
            .unwrap()
            .max,
        [1.0, 1.0, 1.0]
    );
    assert_eq!(analysis.plates[0].instances[1].object_id, beta.id);
    assert_eq!(
        analysis.plates[0].instances[1]
            .printable_bounds
            .unwrap()
            .max,
        [12.0, 22.0, 32.0]
    );
}

#[test]
fn production_build_item_path_must_be_declared_by_model_relationships() {
    let entries = production_build_path_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "3D/_rels/3dmodel.model.rels" {
                (
                    name,
                    MAIN_RELS
                        .replace("/3D/Objects/parts.model", "/3D/Objects/nested.model")
                        .into_bytes(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect();

    assert!(matches!(
        analyze_project(write_archive(entries).path()),
        Err(AnalysisError::InvalidStructure(message))
            if message.contains("external model \"3D/Objects/parts.model\" is not declared")
    ));
}

#[test]
fn instance_ids_select_per_object_build_items_independent_of_metadata_order() {
    const FIRST_INSTANCE: &str = r#"<model_instance><metadata key="object_id" value="10"/><metadata key="instance_id" value="0"/></model_instance>"#;
    const SECOND_INSTANCE: &str = r#"<model_instance><metadata key="object_id" value="10"/><metadata key="instance_id" value="1"/></model_instance>"#;
    const SENTINEL: &str = "__SECOND_OBJECT_TEN_INSTANCE__";

    let reversed_settings = MODEL_SETTINGS
        .replace(FIRST_INSTANCE, SENTINEL)
        .replace(SECOND_INSTANCE, FIRST_INSTANCE)
        .replace(SENTINEL, SECOND_INSTANCE);
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "Metadata/model_settings.config" {
                (name, reversed_settings.as_bytes().to_vec(), method)
            } else {
                (name, data, method)
            }
        })
        .collect();

    let analysis = analyze_project(write_archive(entries).path()).unwrap();
    let instance_one = &analysis.plates[0].instances[0];
    let instance_zero = &analysis.plates[1].instances[0];
    assert_eq!(instance_one.instance_id, 1);
    assert_eq!(
        instance_one.printable_bounds.unwrap().min,
        [23.0, 21.0, 69.0]
    );
    assert_eq!(instance_zero.instance_id, 0);
    assert_eq!(
        instance_zero.printable_bounds.unwrap().min,
        [115.0, 262.0, 318.0]
    );
}

#[test]
fn duplicate_model_settings_objects_with_conflicting_parts_fail_closed() {
    const NEXT_OBJECT: &str = r#"</object>
  <object id="20">"#;
    const CONFLICTING_DUPLICATE: &str = r#"</object>
  <object id="10">
    <metadata key="name" value="Conflicting duplicate"/>
    <metadata key="extruder" value="3"/>
    <part id="201" subtype="normal_part">
      <metadata key="name" value="Conflicting body"/>
      <metadata key="extruder" value="3"/>
    </part>
  </object>
  <object id="20">"#;

    let conflicting_settings = MODEL_SETTINGS.replacen(NEXT_OBJECT, CONFLICTING_DUPLICATE, 1);
    assert_ne!(conflicting_settings, MODEL_SETTINGS);
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "Metadata/model_settings.config" {
                (name, conflicting_settings.as_bytes().to_vec(), method)
            } else {
                (name, data, method)
            }
        })
        .collect();

    assert!(matches!(
        analyze_project(write_archive(entries).path()),
        Err(AnalysisError::InvalidStructure(message))
            if message == "duplicate object ID 10 in Metadata/model_settings.config"
    ));
}

#[test]
fn ambiguous_instance_ids_fail_closed_without_build_transforms_or_bounds() {
    const SECOND_INSTANCE: &str = r#"<model_instance><metadata key="object_id" value="10"/><metadata key="instance_id" value="1"/></model_instance>"#;
    const DUPLICATE_INSTANCE: &str = r#"<model_instance><metadata key="object_id" value="10"/><metadata key="instance_id" value="0"/></model_instance>"#;

    let ambiguous_settings = MODEL_SETTINGS.replace(SECOND_INSTANCE, DUPLICATE_INSTANCE);
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "Metadata/model_settings.config" {
                (name, ambiguous_settings.as_bytes().to_vec(), method)
            } else {
                (name, data, method)
            }
        })
        .collect();

    let analysis = analyze_project(write_archive(entries).path()).unwrap();
    let object_ten_instances: Vec<_> = analysis
        .plates
        .iter()
        .flat_map(|plate| &plate.instances)
        .filter(|instance| instance.object_id == 10)
        .collect();
    assert_eq!(object_ten_instances.len(), 2);
    assert!(object_ten_instances.iter().all(|instance| {
        !instance.printable && instance.transform.is_none() && instance.printable_bounds.is_none()
    }));
    assert!(analysis.warnings.iter().any(|warning| {
        warning.code == WarningCode::UnsafeBuildInstanceGraph
            && warning.object_id == Some(10)
            && warning.message.contains("do not map uniquely")
            && warning.message.contains("excluded from planning")
    }));
}

#[test]
fn rejects_parent_traversal_in_the_central_directory() {
    let mut entries = valid_entries();
    entries.push((
        "../outside",
        b"not extracted".to_vec(),
        CompressionMethod::Stored,
    ));
    let archive = write_archive(entries);
    assert!(matches!(
        analyze_project(archive.path()),
        Err(AnalysisError::UnsafeEntryPath { .. })
    ));
}

#[test]
fn rejects_equivalent_and_prefix_derived_opc_part_names() {
    let mut equivalent_entries = valid_entries();
    equivalent_entries.push((
        "3d/3DMODEL.MODEL",
        MAIN_MODEL.as_bytes().to_vec(),
        CompressionMethod::Stored,
    ));
    let equivalent = write_archive(equivalent_entries);
    assert!(matches!(
        analyze_project(equivalent.path()),
        Err(AnalysisError::EquivalentEntryName { .. })
    ));

    let mut prefix_entries = valid_entries();
    prefix_entries.push(("Metadata", b"part".to_vec(), CompressionMethod::Stored));
    let prefix = write_archive(prefix_entries);
    assert!(matches!(
        analyze_project(prefix.path()),
        Err(AnalysisError::PartNameDerivationConflict { .. })
    ));
}

#[test]
fn configurable_entry_and_total_limits_are_enforced_before_parsing() {
    let archive = write_archive(valid_entries());
    let limits = AnalysisLimits {
        max_entries: 2,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::TooManyEntries { .. })
    ));

    let archive = write_archive(valid_entries());
    let limits = AnalysisLimits {
        max_entry_uncompressed_bytes: 100,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::EntryTooLarge { .. })
    ));

    let archive = write_archive(valid_entries());
    let limits = AnalysisLimits {
        max_total_uncompressed_bytes: 100,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::ArchiveTooLarge { .. })
    ));
}

#[test]
fn advertised_entry_count_is_bounded_before_zip_archive_allocation() {
    let archive = write_archive(valid_entries());
    let mut bytes = std::fs::read(archive.path()).unwrap();
    let eocd = bytes
        .windows(4)
        .rposition(|window| window == [0x50, 0x4b, 0x05, 0x06])
        .unwrap();
    bytes[eocd + 8..eocd + 10].copy_from_slice(&101_u16.to_le_bytes());
    bytes[eocd + 10..eocd + 12].copy_from_slice(&101_u16.to_le_bytes());
    std::fs::write(archive.path(), bytes).unwrap();
    let limits = AnalysisLimits {
        max_entries: 100,
        ..AnalysisLimits::default()
    };

    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::TooManyEntries {
            actual: 101,
            limit: 100
        })
    ));
}

#[test]
fn model_xml_uses_a_separate_streaming_entry_limit() {
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "3D/3dmodel.model" {
                let padding = " ".repeat(8 * 1024);
                (
                    name,
                    MAIN_MODEL
                        .replacen("</model>", &format!("{padding}</model>"), 1)
                        .into_bytes(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect();
    let archive = write_archive(entries);
    let limits = AnalysisLimits {
        max_entry_uncompressed_bytes: 2 * 1024,
        max_model_entry_uncompressed_bytes: 16 * 1024,
        ..AnalysisLimits::default()
    };

    let analysis = analyze_project_with_limits(archive.path(), limits)
        .expect("large model XML should be streamed under its dedicated limit");
    assert_eq!(analysis.summary.object_count, 2);
}

#[test]
fn oversized_single_xml_token_is_rejected_under_the_streaming_entry_limit() {
    let oversized_text = "X".repeat(512);
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "3D/3dmodel.model" {
                (
                    name,
                    MAIN_MODEL
                        .replacen("Synthetic project", &oversized_text, 1)
                        .into_bytes(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect();
    let archive = write_archive(entries);
    let limits = AnalysisLimits {
        max_xml_token_bytes: 384,
        ..AnalysisLimits::default()
    };

    let error = analyze_project_with_limits(archive.path(), limits).unwrap_err();
    assert!(
        matches!(
            &error,
            AnalysisError::InvalidXml { entry, message }
                if entry == "3D/3dmodel.model" && message.contains("XML text token exceeds")
        ),
        "{error:?}"
    );
}

#[test]
fn model_xml_limit_and_non_model_entry_limit_are_enforced_independently() {
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "3D/3dmodel.model" {
                let padding = " ".repeat(8 * 1024);
                (
                    name,
                    MAIN_MODEL
                        .replacen("</model>", &format!("{padding}</model>"), 1)
                        .into_bytes(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect();
    let archive = write_archive(entries);
    let limits = AnalysisLimits {
        max_entry_uncompressed_bytes: 2 * 1024,
        max_model_entry_uncompressed_bytes: 4 * 1024,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::EntryTooLarge { path, limit, .. })
            if path == "3D/3dmodel.model" && limit == 4 * 1024
    ));

    let mut entries = valid_entries();
    entries.push((
        "Metadata/large.bin",
        vec![0x5a; 4 * 1024],
        CompressionMethod::Stored,
    ));
    let archive = write_archive(entries);
    let limits = AnalysisLimits {
        max_entry_uncompressed_bytes: 2 * 1024,
        max_model_entry_uncompressed_bytes: 16 * 1024,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::EntryTooLarge { path, limit, .. })
            if path == "Metadata/large.bin" && limit == 2 * 1024
    ));
}

#[test]
fn legacy_serialized_limits_receive_the_default_model_entry_limit() {
    let legacy = r#"{
        "max_entries": 4096,
        "max_entry_name_bytes": 1024,
        "max_entry_uncompressed_bytes": 536870912,
        "max_total_uncompressed_bytes": 2147483648,
        "max_compression_ratio": 200.0,
        "max_config_bytes": 67108864,
        "max_relationship_bytes": 8388608,
        "max_xml_depth": 256,
        "max_paint_annotation_chars": 1048576
    }"#;

    let limits: AnalysisLimits = serde_json::from_str(legacy).unwrap();
    assert_eq!(
        limits.max_model_entry_uncompressed_bytes,
        1024 * 1024 * 1024
    );
    assert_eq!(limits.max_xml_token_bytes, 8 * 1024 * 1024);
}

#[test]
fn zero_model_entry_limit_is_invalid() {
    let archive = write_archive(valid_entries());
    let limits = AnalysisLimits {
        max_model_entry_uncompressed_bytes: 0,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::InvalidLimits(_))
    ));
}

#[test]
fn compression_ratio_limit_rejects_a_bomb_like_entry() {
    let mut entries = valid_entries();
    entries.push((
        "Metadata/repeated.bin",
        vec![0; 1024 * 1024],
        CompressionMethod::Deflated,
    ));
    let archive = write_archive(entries);
    let limits = AnalysisLimits {
        max_compression_ratio: 10.0,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::CompressionRatioExceeded { .. })
    ));
}

#[test]
fn missing_root_relationship_is_rejected() {
    let entries = valid_entries()
        .into_iter()
        .filter(|(name, _, _)| *name != "_rels/.rels")
        .collect();
    let archive = write_archive(entries);
    assert!(matches!(
        analyze_project(archive.path()),
        Err(AnalysisError::MissingRequiredEntry(path)) if path == "_rels/.rels"
    ));
}

#[test]
fn missing_external_model_relationship_is_rejected() {
    let entries = valid_entries()
        .into_iter()
        .filter(|(name, _, _)| *name != "3D/_rels/3dmodel.model.rels")
        .collect();
    let archive = write_archive(entries);
    assert!(matches!(
        analyze_project(archive.path()),
        Err(AnalysisError::MissingRequiredEntry(path))
            if path == "3D/_rels/3dmodel.model.rels"
    ));
}

#[test]
fn dangling_component_reference_is_reported() {
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "3D/3dmodel.model" {
                (
                    name,
                    MAIN_MODEL
                        .replacen("objectid=\"100\"", "objectid=\"999\"", 1)
                        .into_bytes(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect();
    let archive = write_archive(entries);
    let analysis = analyze_project(archive.path()).unwrap();
    assert!(analysis.warnings.iter().any(|warning| {
        warning.code == WarningCode::UnsafeBuildInstanceGraph
            && warning.object_id == Some(10)
            && warning.entry_path.as_deref() == Some("3D/Objects/parts.model")
            && warning
                .message
                .contains("references missing resource object 999")
    }));
}

#[test]
fn oversized_metadata_xml_is_rejected() {
    let entries = valid_entries()
        .into_iter()
        .filter(|(name, _, _)| *name != "Metadata/project_settings.config")
        .collect();
    let archive = write_archive(entries);
    let limits = AnalysisLimits {
        max_config_bytes: 100,
        ..AnalysisLimits::default()
    };
    assert!(matches!(
        analyze_project_with_limits(archive.path(), limits),
        Err(AnalysisError::MetadataEntryTooLarge { path, .. })
            if path == "Metadata/model_settings.config"
    ));
}

#[test]
fn doctype_is_rejected_without_entity_expansion() {
    let entries = valid_entries()
        .into_iter()
        .map(|(name, data, method)| {
            if name == "3D/3dmodel.model" {
                (
                    name,
                    MAIN_MODEL
                        .replacen(
                            "<model",
                            "<!DOCTYPE model [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]><model",
                            1,
                        )
                        .into_bytes(),
                    method,
                )
            } else {
                (name, data, method)
            }
        })
        .collect();
    let archive = write_archive(entries);
    assert!(matches!(
        analyze_project(archive.path()),
        Err(AnalysisError::ForbiddenDoctype(path)) if path == "3D/3dmodel.model"
    ));
}

#[test]
fn crc_corruption_of_a_read_entry_is_detected() {
    let archive = write_archive(valid_entries());
    let mut bytes = std::fs::read(archive.path()).unwrap();
    let marker = b"Synthetic project";
    let position = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("stored XML marker");
    bytes[position] = b'X';
    std::fs::write(archive.path(), bytes).unwrap();

    assert!(analyze_project(archive.path()).is_err());
}
