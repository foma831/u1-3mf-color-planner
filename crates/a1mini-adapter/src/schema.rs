use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use u1_planner::{Material, RgbColor};

use crate::profiles::ResolvedTargetProfiles;
use crate::{
    A1MINI_APPLICATION_VERSION, A1MINI_BED_DEPTH_MM, A1MINI_BED_WIDTH_MM, A1MINI_MACHINE_PROFILE,
    A1MINI_MACHINE_SETTING_ID, A1MINI_NOZZLE_DIAMETER_MM, A1MINI_PRINTABLE_HEIGHT_MM,
    A1MINI_PROCESS_PROFILE, A1MINI_PROCESS_SETTING_ID, A1MiniError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A1MiniMaterial {
    Pla,
    Petg,
}

impl A1MiniMaterial {
    #[must_use]
    pub const fn profile_name(self) -> &'static str {
        match self {
            Self::Pla => "Generic PLA @BBL A1M",
            Self::Petg => "Generic PETG @BBL A1M",
        }
    }

    #[must_use]
    pub const fn material_name(self) -> &'static str {
        match self {
            Self::Pla => "PLA",
            Self::Petg => "PETG",
        }
    }
}

impl TryFrom<&Material> for A1MiniMaterial {
    type Error = A1MiniError;

    fn try_from(value: &Material) -> Result<Self, Self::Error> {
        match value {
            Material::Pla => Ok(Self::Pla),
            Material::Petg => Ok(Self::Petg),
            other => Err(A1MiniError::Plan(format!(
                "material {other:?} has no qualified A1 mini single-spool profile"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniSpoolSpec {
    pub spool_id: String,
    pub display_name: String,
    pub material: A1MiniMaterial,
    pub color: RgbColor,
    pub material_substitution_approval_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniTargetSchema {
    pub application_version: String,
    pub machine_profile: String,
    pub machine_setting_id: String,
    pub process_profile: String,
    pub process_setting_id: String,
    pub nozzle_diameter_mm: f64,
    pub bed_width_mm: f64,
    pub bed_depth_mm: f64,
    pub printable_height_mm: f64,
    pub logical_slot_count: usize,
    pub has_filament_switcher: bool,
    pub single_plate_only: bool,
}

impl Default for A1MiniTargetSchema {
    fn default() -> Self {
        Self {
            application_version: A1MINI_APPLICATION_VERSION.into(),
            machine_profile: A1MINI_MACHINE_PROFILE.into(),
            machine_setting_id: A1MINI_MACHINE_SETTING_ID.into(),
            process_profile: A1MINI_PROCESS_PROFILE.into(),
            process_setting_id: A1MINI_PROCESS_SETTING_ID.into(),
            nozzle_diameter_mm: A1MINI_NOZZLE_DIAMETER_MM,
            bed_width_mm: A1MINI_BED_WIDTH_MM,
            bed_depth_mm: A1MINI_BED_DEPTH_MM,
            printable_height_mm: A1MINI_PRINTABLE_HEIGHT_MM,
            logical_slot_count: 1,
            has_filament_switcher: false,
            single_plate_only: true,
        }
    }
}

pub(crate) fn build_project_settings(
    profiles: ResolvedTargetProfiles,
    spool: &A1MiniSpoolSpec,
) -> Result<Vec<u8>, A1MiniError> {
    if profiles.filament.name != spool.material.profile_name() {
        return Err(A1MiniError::Capability(format!(
            "resolved filament profile {:?} does not match target material {}",
            profiles.filament.name,
            spool.material.material_name()
        )));
    }
    let mut settings = profiles.machine;
    settings.extend(profiles.process);
    for key in [
        "type",
        "name",
        "setting_id",
        "inherits",
        "compatible_printers",
        "compatible_printers_condition",
        "instantiation",
        "description",
    ] {
        settings.remove(key);
    }
    let filament_keys = profiles
        .filament
        .values
        .iter()
        .filter_map(|(key, value)| value.is_array().then_some(key.clone()))
        .collect::<BTreeSet<_>>();
    for key in filament_keys {
        if matches!(
            key.as_str(),
            "compatible_printers" | "compatible_prints" | "default_filament_profile"
        ) {
            continue;
        }
        let value = profiles
            .filament
            .values
            .get(&key)
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()));
        settings.insert(key, Value::Array(vec![value]));
    }
    let color = rgb_hex(spool.color);
    settings.insert(
        "printer_model".into(),
        Value::String("Bambu Lab A1 mini".into()),
    );
    settings.insert("printer_variant".into(), Value::String("0.4".into()));
    settings.insert(
        "printer_settings_id".into(),
        Value::String(A1MINI_MACHINE_PROFILE.into()),
    );
    settings.insert(
        "print_settings_id".into(),
        Value::String(A1MINI_PROCESS_PROFILE.into()),
    );
    settings.insert(
        "nozzle_diameter".into(),
        Value::Array(vec![Value::String("0.4".into())]),
    );
    settings.insert(
        "printable_area".into(),
        Value::Array(
            ["0x0", "180x0", "180x180", "0x180"]
                .into_iter()
                .map(|value| Value::String(value.into()))
                .collect(),
        ),
    );
    settings.insert("printable_height".into(), Value::String("180".into()));
    settings.insert("bed_exclude_area".into(), Value::Array(Vec::new()));
    settings.insert("has_filament_switcher".into(), Value::String("0".into()));
    settings.insert(
        "filament_settings_id".into(),
        Value::Array(vec![Value::String(profiles.filament.name.clone())]),
    );
    settings.insert(
        "filament_ids".into(),
        Value::Array(vec![Value::String(profiles.filament.setting_id.clone())]),
    );
    settings.insert(
        "filament_colour".into(),
        Value::Array(vec![Value::String(color.clone())]),
    );
    settings.insert(
        "filament_multi_colour".into(),
        Value::Array(vec![Value::String(color)]),
    );
    settings.insert(
        "filament_type".into(),
        Value::Array(vec![Value::String(spool.material.material_name().into())]),
    );
    settings.insert(
        "default_filament_profile".into(),
        Value::Array(vec![Value::String(profiles.filament.name.clone())]),
    );
    settings.insert(
        "filament_map".into(),
        Value::Array(vec![Value::String("1".into())]),
    );
    settings.insert(
        "filament_nozzle_map".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
    settings.insert(
        "filament_self_index".into(),
        Value::Array(vec![Value::String("1".into())]),
    );
    settings.insert(
        "filament_is_mixed".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
    settings.insert(
        "filament_mixed_components".into(),
        Value::Array(vec![Value::String(String::new())]),
    );
    settings.insert(
        "filament_mixed_gradient".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
    settings.insert(
        "filament_mixed_sublayer_ratios".into(),
        Value::Array(vec![Value::String(String::new())]),
    );
    settings.insert(
        "flush_volumes_matrix".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
    settings.insert(
        "flush_volumes_vector".into(),
        Value::Array(vec![
            Value::String("140".into()),
            Value::String("140".into()),
        ]),
    );
    settings.insert(
        "flush_multiplier".into(),
        Value::Array(vec![Value::String("1".into())]),
    );
    settings.insert(
        "different_settings_to_system".into(),
        Value::Array((0..3).map(|_| Value::String(String::new())).collect()),
    );
    settings.insert(
        "curr_bed_type".into(),
        Value::String("Textured PEI Plate".into()),
    );
    settings.insert("brim_type".into(), Value::String("auto_brim".into()));
    settings.insert("print_sequence".into(), Value::String("by layer".into()));
    settings.insert(
        "first_layer_print_sequence".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
    settings.insert(
        "other_layers_print_sequence".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
    settings.insert(
        "other_layers_print_sequence_nums".into(),
        Value::String("0".into()),
    );
    settings.insert("spiral_mode".into(), Value::String("0".into()));
    settings.insert("spiral_mode_smooth".into(), Value::String("0".into()));
    settings.insert("timelapse_type".into(), Value::String("0".into()));
    settings.insert(
        "wipe_tower_x".into(),
        Value::Array(vec![Value::String("15".into())]),
    );
    settings.insert(
        "wipe_tower_y".into(),
        Value::Array(vec![Value::String("145".into())]),
    );
    settings.insert("from".into(), Value::String("project".into()));
    settings.insert(
        "version".into(),
        Value::String(A1MINI_APPLICATION_VERSION.into()),
    );
    validate_project_settings_map(&settings, spool.material)?;
    let mut bytes = serde_json::to_vec_pretty(&settings).map_err(|source| A1MiniError::Json {
        path: "Metadata/project_settings.config".into(),
        source,
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn validate_a1mini_project_settings(
    bytes: &[u8],
    material: A1MiniMaterial,
) -> Result<(), A1MiniError> {
    let settings = serde_json::from_slice::<BTreeMap<String, Value>>(bytes).map_err(|source| {
        A1MiniError::Json {
            path: "Metadata/project_settings.config".into(),
            source,
        }
    })?;
    validate_project_settings_map(&settings, material)
}

fn validate_project_settings_map(
    settings: &BTreeMap<String, Value>,
    material: A1MiniMaterial,
) -> Result<(), A1MiniError> {
    let scalar = |key: &str| settings.get(key).and_then(Value::as_str);
    let array = |key: &str| settings.get(key).and_then(Value::as_array);
    if scalar("printer_model") != Some("Bambu Lab A1 mini")
        || scalar("printer_variant") != Some("0.4")
        || scalar("printer_settings_id") != Some(A1MINI_MACHINE_PROFILE)
        || scalar("print_settings_id") != Some(A1MINI_PROCESS_PROFILE)
        || scalar("printable_height") != Some("180")
        || scalar("has_filament_switcher") != Some("0")
        || scalar("version") != Some(A1MINI_APPLICATION_VERSION)
    {
        return Err(A1MiniError::Capability(
            "generated project does not match the exact A1 mini target identity".into(),
        ));
    }
    for key in [
        "nozzle_diameter",
        "filament_settings_id",
        "filament_ids",
        "filament_colour",
        "filament_type",
        "filament_map",
        "filament_nozzle_map",
        "filament_self_index",
        "filament_is_mixed",
        "flush_volumes_matrix",
    ] {
        if array(key).is_none_or(|values| values.len() != 1) {
            return Err(A1MiniError::Capability(format!(
                "generated A1 mini setting {key} must contain exactly one logical slot"
            )));
        }
    }
    if array("nozzle_diameter")
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        != Some("0.4")
        || array("filament_settings_id")
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            != Some(material.profile_name())
        || array("filament_type")
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            != Some(material.material_name())
        || array("filament_map")
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            != Some("1")
        || array("filament_nozzle_map")
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            != Some("0")
        || array("filament_is_mixed")
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            != Some("0")
        || array("flush_volumes_matrix")
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            != Some("0")
    {
        return Err(A1MiniError::Capability(
            "generated project violates one-slot/no-AMS material semantics".into(),
        ));
    }
    let printable_area = array("printable_area")
        .ok_or_else(|| A1MiniError::Capability("generated project has no printable_area".into()))?;
    let expected_area = ["0x0", "180x0", "180x180", "0x180"];
    if printable_area.len() != expected_area.len()
        || printable_area
            .iter()
            .zip(expected_area)
            .any(|(actual, expected)| actual.as_str() != Some(expected))
    {
        return Err(A1MiniError::Capability(
            "generated project does not use the qualified 180 x 180 mm bed polygon".into(),
        ));
    }
    Ok(())
}

fn rgb_hex(color: RgbColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_materials_outside_the_qualified_pair() {
        assert_eq!(
            A1MiniMaterial::try_from(&Material::Pla).unwrap(),
            A1MiniMaterial::Pla
        );
        assert_eq!(
            A1MiniMaterial::try_from(&Material::Petg).unwrap(),
            A1MiniMaterial::Petg
        );
        assert!(A1MiniMaterial::try_from(&Material::Abs).is_err());
    }

    #[test]
    fn schema_is_explicitly_single_plate_single_slot_no_ams() {
        let schema = A1MiniTargetSchema::default();
        assert_eq!(schema.logical_slot_count, 1);
        assert!(!schema.has_filament_switcher);
        assert!(schema.single_plate_only);
        assert_eq!(schema.bed_width_mm, 180.0);
        assert_eq!(schema.nozzle_diameter_mm, 0.4);
    }
}
