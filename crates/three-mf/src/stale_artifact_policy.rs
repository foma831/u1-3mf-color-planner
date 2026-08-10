//! Classification and policy for derived artifacts copied from a source 3MF.
//!
//! The writer can use [`classify_stale_entry`] while assembling a package to
//! decide which source entries must be dropped or regenerated. The structural
//! output validator applies [`StaleArtifactPolicy`] again before publication so
//! an unsliced project cannot accidentally retain toolpaths or slice metadata.

use serde::{Deserialize, Serialize};

/// The intended kind of the produced 3MF package.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputArtifactProfile {
    /// An editable project that must not contain toolpaths or slice results.
    #[default]
    UnslicedProject,
    /// A package intentionally containing slicer output.
    SlicedProject,
}

/// A derived entry whose source copy can become stale after conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleArtifactKind {
    Toolpath,
    ToolpathChecksum,
    SliceMetadata,
    SlicePreview,
    Thumbnail,
}

/// What a package builder or validator should do with a classified entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleArtifactDisposition {
    Preserve,
    Reject,
    RegenerateOrRemove,
    RequireRelationship,
}

/// Deterministic classification returned for a stale-sensitive package entry.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleEntryClassification {
    pub path: String,
    pub kind: StaleArtifactKind,
    pub disposition: StaleArtifactDisposition,
}

/// Policy used both while rewriting entries and while validating staged output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StaleArtifactPolicy {
    pub output_profile: OutputArtifactProfile,
    /// Without provenance, a copied plate preview cannot be distinguished from
    /// a newly rendered one. Keep this strict unless the writer regenerated it.
    pub allow_regenerated_slice_previews: bool,
}

impl Default for StaleArtifactPolicy {
    fn default() -> Self {
        Self {
            output_profile: OutputArtifactProfile::UnslicedProject,
            allow_regenerated_slice_previews: false,
        }
    }
}

impl StaleArtifactPolicy {
    pub fn unsliced() -> Self {
        Self::default()
    }

    pub fn sliced() -> Self {
        Self {
            output_profile: OutputArtifactProfile::SlicedProject,
            allow_regenerated_slice_previews: true,
        }
    }

    pub fn disposition_for(&self, kind: StaleArtifactKind) -> StaleArtifactDisposition {
        match (self.output_profile, kind) {
            (OutputArtifactProfile::UnslicedProject, StaleArtifactKind::Toolpath)
            | (OutputArtifactProfile::UnslicedProject, StaleArtifactKind::ToolpathChecksum)
            | (OutputArtifactProfile::UnslicedProject, StaleArtifactKind::SliceMetadata) => {
                StaleArtifactDisposition::Reject
            }
            (OutputArtifactProfile::UnslicedProject, StaleArtifactKind::SlicePreview)
                if !self.allow_regenerated_slice_previews =>
            {
                StaleArtifactDisposition::RegenerateOrRemove
            }
            (_, StaleArtifactKind::Thumbnail) => StaleArtifactDisposition::RequireRelationship,
            _ => StaleArtifactDisposition::Preserve,
        }
    }

    pub fn classify(&self, path: &str) -> Option<StaleEntryClassification> {
        let kind = stale_artifact_kind(path)?;
        Some(StaleEntryClassification {
            path: path.to_owned(),
            kind,
            disposition: self.disposition_for(kind),
        })
    }

    pub fn classify_entries<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Vec<StaleEntryClassification> {
        let mut classifications = paths
            .into_iter()
            .filter_map(|path| self.classify(path))
            .collect::<Vec<_>>();
        classifications.sort();
        classifications.dedup();
        classifications
    }
}

/// Classifies a stale-sensitive entry using the strict unsliced policy.
pub fn classify_stale_entry(path: &str) -> Option<StaleEntryClassification> {
    StaleArtifactPolicy::unsliced().classify(path)
}

/// Returns only the intrinsic artifact kind, independent of output policy.
pub fn stale_artifact_kind(path: &str) -> Option<StaleArtifactKind> {
    let normalized = path.trim_start_matches('/').replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    if is_toolpath_checksum(file_name, &lower) {
        return Some(StaleArtifactKind::ToolpathChecksum);
    }
    if is_toolpath(file_name, &lower) {
        return Some(StaleArtifactKind::Toolpath);
    }
    if is_thumbnail(file_name, &lower) {
        return Some(StaleArtifactKind::Thumbnail);
    }
    if is_slice_metadata(file_name, &lower) {
        return Some(StaleArtifactKind::SliceMetadata);
    }
    if is_slice_preview(file_name, &lower) {
        return Some(StaleArtifactKind::SlicePreview);
    }
    None
}

fn is_toolpath(file_name: &str, path: &str) -> bool {
    file_name.ends_with(".gcode")
        || file_name.ends_with(".bgcode")
        || file_name.contains(".gcode.")
        || file_name.contains(".bgcode.")
        || path.starts_with("gcode/")
        || path.contains("/gcode/")
}

fn is_toolpath_checksum(file_name: &str, path: &str) -> bool {
    let is_checksum = file_name.ends_with(".md5")
        || file_name.ends_with(".sha1")
        || file_name.ends_with(".sha256")
        || file_name.ends_with(".checksum");
    is_checksum && (file_name.contains("gcode") || path.contains("toolpath"))
}

fn is_thumbnail(file_name: &str, path: &str) -> bool {
    path.contains("/.thumbnails/")
        || path.starts_with(".thumbnails/")
        || file_name.starts_with("thumbnail")
        || file_name.contains("_thumbnail")
        || file_name.contains("thumbnail_")
}

fn is_slice_metadata(file_name: &str, path: &str) -> bool {
    matches!(
        path,
        "metadata/slice_info.config"
            | "metadata/slice_data.config"
            | "metadata/filament_sequence.json"
            | "metadata/layer_height_profile.txt"
    ) || (path.starts_with("metadata/plate_") && file_name.ends_with(".json"))
        || file_name.starts_with("slice_info")
        || file_name.starts_with("slice_data")
}

fn is_slice_preview(file_name: &str, path: &str) -> bool {
    let is_image = [".png", ".jpg", ".jpeg", ".webp"]
        .iter()
        .any(|extension| file_name.ends_with(extension));
    is_image
        && path.starts_with("metadata/")
        && (file_name.starts_with("top_")
            || file_name.starts_with("pick_")
            || file_name.starts_with("plate_")
            || file_name.starts_with("plate_no_light_"))
}
