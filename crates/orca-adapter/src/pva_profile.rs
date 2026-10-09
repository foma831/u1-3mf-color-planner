use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) const RELI3D_PVA_PROFILE_NAME: &str = "Reli3D PVA @U1";
pub(crate) const RELI3D_PVA_SETTING_ID: &str = "U1PVA-RELI3D-001";
pub(crate) const RELI3D_PVA_FILAMENT_ID: &str = "U1PVA-RELI3D";

/// Applies the qualified Reli3D PVA overrides to the verified Snapmaker PVA
/// inheritance chain. The source profile remains the structural baseline;
/// these values define a distinct planner-owned identity and never pretend to
/// be the Snapmaker material preset.
pub(crate) fn apply_reli3d_pva_profile(settings: &mut BTreeMap<String, Value>) {
    settings.insert("name".into(), Value::String(RELI3D_PVA_PROFILE_NAME.into()));
    settings.insert(
        "setting_id".into(),
        Value::String(RELI3D_PVA_SETTING_ID.into()),
    );
    settings.insert(
        "filament_id".into(),
        Value::String(RELI3D_PVA_FILAMENT_ID.into()),
    );

    for (key, value) in [
        ("nozzle_temperature_range_low", "190"),
        ("nozzle_temperature_range_high", "230"),
        ("nozzle_temperature", "210"),
        ("nozzle_temperature_initial_layer", "220"),
        ("hot_plate_temp", "60"),
        ("hot_plate_temp_initial_layer", "60"),
        ("textured_plate_temp", "60"),
        ("textured_plate_temp_initial_layer", "60"),
        ("graphic_effect_plate_temp", "60"),
        ("graphic_effect_plate_temp_initial_layer", "60"),
        ("filament_max_volumetric_speed", "3"),
        ("filament_retraction_length", "1"),
        ("filament_retraction_speed", "25"),
        ("filament_deretraction_speed", "20"),
        ("slow_down_layer_time", "12"),
    ] {
        settings.insert(key.into(), Value::Array(vec![Value::String(value.into())]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reli3d_profile_stays_within_the_published_temperature_range() {
        let mut settings = BTreeMap::new();
        apply_reli3d_pva_profile(&mut settings);

        let value = |key: &str| {
            settings[key].as_array().unwrap()[0]
                .as_str()
                .unwrap()
                .parse::<u16>()
                .unwrap()
        };
        assert_eq!(value("nozzle_temperature_range_low"), 190);
        assert_eq!(value("nozzle_temperature_range_high"), 230);
        assert!((190..=230).contains(&value("nozzle_temperature")));
        assert!((190..=230).contains(&value("nozzle_temperature_initial_layer")));
        assert!((45..=65).contains(&value("textured_plate_temp")));
    }
}
