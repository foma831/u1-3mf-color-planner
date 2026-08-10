//! Clean-room Bambu Lab A1 mini single-spool Project 3MF adapter.
//!
//! The adapter is intentionally version-scoped and fail-closed. It accepts
//! one physically packed A1 mini plate per output file, collapses every
//! approved source material/color assignment to one logical slot, and emits
//! an unsliced no-AMS Project 3MF through the generic bounded OPC writer.

mod a1_round_trip;
mod capability;
mod profiles;
mod schema;
mod writer;

pub use a1_round_trip::*;
pub use capability::*;
pub use schema::*;
pub use writer::*;

pub const A1MINI_ADAPTER_ID: &str = "bambu-studio/02.02.00.85/a1-mini-0.4-no-ams";
/// Stable prefix used by callers to classify cooperative conversion cancellation.
pub const A1MINI_CANCELLATION_ERROR_PREFIX: &str = "conversion_cancelled:";
pub const A1MINI_APPLICATION_VERSION: &str = "02.02.00.85";
pub const A1MINI_EXECUTABLE_SHA256: &str =
    "88b72ac8523f36f4be0c5aafe99337baa6f596f502f9dfb4356ad2617c1b7458";
pub const A1MINI_PROFILE_PACK_VERSION: &str = "02.02.00.02";
pub const A1MINI_PROFILE_MANIFEST_SHA256: &str =
    "2b34f3a24718cee1dcbd83250d840010d95d5bb08d27fb0fc0c8da4c02b39feb";
pub const A1MINI_MACHINE_PROFILE: &str = "Bambu Lab A1 mini 0.4 nozzle";
pub const A1MINI_MACHINE_SETTING_ID: &str = "GM020";
pub const A1MINI_PROCESS_PROFILE: &str = "0.20mm Standard @BBL A1M";
pub const A1MINI_PROCESS_SETTING_ID: &str = "GP000";
pub const A1MINI_PROJECT_SCHEMA_VERSION: &str = "1";
pub const A1MINI_BED_WIDTH_MM: f64 = 180.0;
pub const A1MINI_BED_DEPTH_MM: f64 = 180.0;
pub const A1MINI_PRINTABLE_HEIGHT_MM: f64 = 180.0;
pub const A1MINI_NOZZLE_DIAMETER_MM: f64 = 0.4;

const BBL_VENDOR_NAME: &str = "BBL";
const BBL_SYSTEM_DIRECTORY: &str = "system";
const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";

const MACHINE_PROFILE_PATH: &str = "machine/Bambu Lab A1 mini 0.4 nozzle.json";
const PROCESS_PROFILE_PATH: &str = "process/0.20mm Standard @BBL A1M.json";
const GENERIC_PLA_PROFILE_PATH: &str = "filament/Generic PLA @BBL A1M.json";
const GENERIC_PETG_PROFILE_PATH: &str = "filament/Generic PETG @BBL A1M.json";

const A1MINI_PROFILE_BASELINE: &[(&str, &str)] = &[
    (
        MACHINE_PROFILE_PATH,
        "eb2503620defa46a7564a92b66f59c7bb27eaa8a40afbc87dcc84908fd81fa0a",
    ),
    (
        "machine/fdm_bbl_3dp_001_common.json",
        "428e85a328d7f04310db6b88c5c85d7e888ce61aa41a7dac26bdd110df36a7f2",
    ),
    (
        "machine/fdm_machine_common.json",
        "a0eaa507084eb9ba69207498b9c84766912e1521f668828181685e9fcb45b8a0",
    ),
    (
        PROCESS_PROFILE_PATH,
        "e804557e4d014d1e726ad6c0101f3bb627466b9c7adbf445ece060dfb4af4415",
    ),
    (
        "process/0.20mm Standard @BBL P1P.json",
        "eb93e7d484b07b2a4b104786d3bee4b9abb02cd027245106b3936ee1671a7e48",
    ),
    (
        "process/fdm_process_single_0.20.json",
        "cdda8d56d67f62a0faa892cb6efc605ebb5c745bf742f6ead4daaeb6627c182a",
    ),
    (
        "process/fdm_process_single_common.json",
        "05b38ac1318e465e817978b36da51754f928da0172738a20bb3a8d4a69eb4769",
    ),
    (
        "process/fdm_process_common.json",
        "68a0dde1513c863bae9b5bbd017e333b7a4361e7d6d0d13773be05695b9dbe91",
    ),
    (
        GENERIC_PLA_PROFILE_PATH,
        "24286e1c65a757bfc400cec200aa66af99c61b099b271bc399cc176417e627f1",
    ),
    (
        "filament/Generic PLA @base.json",
        "530b745e2062c4c577d7acce3601ac7586fc65de394726252354bec0ade3e4c5",
    ),
    (
        "filament/fdm_filament_pla.json",
        "3ae5cdf6f02587cac56b7ba358b56d14ebe119295dfd17a3a3a57717a2fa0f79",
    ),
    (
        GENERIC_PETG_PROFILE_PATH,
        "6190f0d0085a94c521692389d5961fa082c414ef9c07acb44d54805a34e3c636",
    ),
    (
        "filament/Generic PETG @base.json",
        "6f6c6f213537cf75ab87f5eae7454a876ca268208d70f6b241c5396224a95b1a",
    ),
    (
        "filament/fdm_filament_pet.json",
        "1b15d3c8f7ca64c79ee5e68eee5368e1ee8b128f9c381f076832f0123d1b6023",
    ),
    (
        "filament/fdm_filament_common.json",
        "da928e89a0694867187a346edce8eb8af515e8ebe598e18929cb2666192a19d9",
    ),
];
