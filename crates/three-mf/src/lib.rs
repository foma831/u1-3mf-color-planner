//! Safe parsing, deterministic package construction, and structural validation
//! for Bambu Studio and OrcaSlicer project 3MF archives.
//!
//! The crate never extracts an archive. It validates ZIP paths and resource
//! limits before streaming metadata, model XML, and explicitly selected entries.

mod analyzer;
mod bounded_xml;
mod deterministic_ids;
mod opc;
mod opc_writer;
mod output_validation;
mod package_manifest;
mod paint;
mod stale_artifact_policy;
mod types;
mod zip_preflight;

pub use analyzer::{
    AnalysisError, Analyzer, OrientationWriteError, analyze_project, analyze_project_with_limits,
    optimize_object_orientation, optimize_plate_orientations, write_optimized_object_orientation,
    write_optimized_plate_orientations, write_optimized_plate_reports,
};
pub use deterministic_ids::{DeterministicIdError, DeterministicProductionIds};
pub use opc::*;
pub use opc_writer::{
    ExpectedSourceIdentity, OpcEntryOrigin, OpcEntryWriteReport, OpcPackageWriter, OpcWriteError,
    OpcWriteLimits, OpcWriteReport, StagedPackage, StagedPackageValidationError,
    ValidatedStagedPackage, VerifiedSourceIdentity,
};
pub use output_validation::*;
pub use package_manifest::*;
pub use paint::{
    MAX_PAINT_STATE, PaintCodecError, PaintNode, decode_paint_annotation, encode_paint_annotation,
    used_paint_states,
};
pub use stale_artifact_policy::*;
pub use types::*;
