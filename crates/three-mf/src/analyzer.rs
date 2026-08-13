use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::bounded_xml::XmlTokenLimitedReader;
use crate::paint::{PaintCodecError, used_paint_states};
use crate::stale_artifact_policy::{StaleArtifactKind, stale_artifact_kind};
use crate::types::*;
use crate::zip_preflight::advertised_zip_entry_count;

const CONTENT_TYPES_PATH: &str = "[Content_Types].xml";
const ROOT_RELATIONSHIPS_PATH: &str = "_rels/.rels";
const MAIN_MODEL_PATH: &str = "3D/3dmodel.model";
const MAIN_RELATIONSHIPS_PATH: &str = "3D/_rels/3dmodel.model.rels";
const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const MAX_COMPONENT_GRAPH_DEPTH: usize = 256;
const MAX_SOURCE_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MODEL_XML_BUFFER_BYTES: usize = 128 * 1024;

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("failed to access the input: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid ZIP container: {0}")]
    Zip(#[from] ZipError),
    #[error("invalid ZIP end records: {0}")]
    InvalidZipEndRecords(String),
    #[error("input does not start with a ZIP signature")]
    InvalidZipSignature,
    #[error("input is not a regular file: {0:?}")]
    InputNotRegularFile(PathBuf),
    #[error("input archive is {actual} bytes; limit is {limit} bytes")]
    InputArchiveTooLarge { actual: u64, limit: u64 },
    #[error("input path or file changed while it was being analyzed: {0:?}")]
    SourceChangedDuringAnalysis(PathBuf),
    #[error("analysis limits are invalid: {0}")]
    InvalidLimits(String),
    #[error("ZIP contains {actual} entries; limit is {limit}")]
    TooManyEntries { actual: usize, limit: usize },
    #[error("unsafe ZIP entry path {path:?}: {reason}")]
    UnsafeEntryPath { path: String, reason: String },
    #[error("ZIP entry name is not valid UTF-8")]
    NonUtf8EntryName,
    #[error("duplicate ZIP entry {0:?}")]
    DuplicateEntry(String),
    #[error("ZIP contains ASCII case-equivalent OPC entries {first:?} and {second:?}")]
    EquivalentEntryName { first: String, second: String },
    #[error("ZIP contains prefix-derived OPC entries {first:?} and {second:?}")]
    PartNameDerivationConflict { first: String, second: String },
    #[error("encrypted ZIP entry is not supported: {0}")]
    EncryptedEntry(String),
    #[error("symbolic-link ZIP entry is not allowed: {0}")]
    SymbolicLinkEntry(String),
    #[error("ZIP entry {path:?} expands to {actual} bytes; limit is {limit}")]
    EntryTooLarge {
        path: String,
        actual: u64,
        limit: u64,
    },
    #[error("ZIP expands to {actual} bytes; limit is {limit}")]
    ArchiveTooLarge { actual: u64, limit: u64 },
    #[error("ZIP entry {path:?} has compression ratio {actual:.2}; limit is {limit:.2}")]
    CompressionRatioExceeded {
        path: String,
        actual: f64,
        limit: f64,
    },
    #[error("required package entry is missing: {0}")]
    MissingRequiredEntry(String),
    #[error("referenced package entry is missing: {0}")]
    MissingReferencedEntry(String),
    #[error("package entry {path:?} exceeds its metadata limit of {limit} bytes")]
    MetadataEntryTooLarge { path: String, limit: u64 },
    #[error("invalid XML in {entry:?}: {message}")]
    InvalidXml { entry: String, message: String },
    #[error("DTD/entity declarations are forbidden in {0:?}")]
    ForbiddenDoctype(String),
    #[error("invalid JSON in {entry:?}: {message}")]
    InvalidJson { entry: String, message: String },
    #[error("invalid paint annotation in {entry:?}: {source}")]
    InvalidPaintAnnotation {
        entry: String,
        #[source]
        source: PaintCodecError,
    },
    #[error("invalid project structure: {0}")]
    InvalidStructure(String),
}

#[derive(Clone, Debug)]
pub struct Analyzer {
    limits: AnalysisLimits,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new(AnalysisLimits::default()).expect("default analysis limits are valid")
    }
}

impl Analyzer {
    pub fn new(limits: AnalysisLimits) -> Result<Self, AnalysisError> {
        validate_limits(&limits)?;
        Ok(Self { limits })
    }

    pub fn limits(&self) -> &AnalysisLimits {
        &self.limits
    }

    pub fn analyze_path(&self, path: impl AsRef<Path>) -> Result<ProjectAnalysis, AnalysisError> {
        analyze_path(path.as_ref(), &self.limits)
    }
}

pub fn analyze_project(path: impl AsRef<Path>) -> Result<ProjectAnalysis, AnalysisError> {
    Analyzer::default().analyze_path(path)
}

pub fn analyze_project_with_limits(
    path: impl AsRef<Path>,
    limits: AnalysisLimits,
) -> Result<ProjectAnalysis, AnalysisError> {
    Analyzer::new(limits)?.analyze_path(path)
}

fn validate_limits(limits: &AnalysisLimits) -> Result<(), AnalysisError> {
    if limits.max_entries == 0
        || limits.max_entry_name_bytes == 0
        || limits.max_entry_uncompressed_bytes == 0
        || limits.max_model_entry_uncompressed_bytes == 0
        || limits.max_total_uncompressed_bytes == 0
        || limits.max_config_bytes == 0
        || limits.max_relationship_bytes == 0
        || limits.max_xml_depth == 0
        || limits.max_xml_token_bytes == 0
        || limits.max_paint_annotation_chars == 0
        || !limits.max_compression_ratio.is_finite()
        || limits.max_compression_ratio < 1.0
    {
        return Err(AnalysisError::InvalidLimits(
            "all count/size limits must be positive and compression ratio must be finite and at least 1"
                .into(),
        ));
    }
    Ok(())
}

fn analyze_path(path: &Path, limits: &AnalysisLimits) -> Result<ProjectAnalysis, AnalysisError> {
    let snapshotted = snapshot_input(path, MAX_SOURCE_ARCHIVE_BYTES)?;
    let byte_size = snapshotted.identity.byte_size;
    let sha256 = snapshotted.identity.sha256.clone();
    let source_guard = snapshotted.source_guard;
    let mut snapshot = snapshotted.snapshot;
    let advertised_entries =
        advertised_zip_entry_count(&mut snapshot).map_err(AnalysisError::InvalidZipEndRecords)?;
    let max_entries = u64::try_from(limits.max_entries).unwrap_or(u64::MAX);
    if advertised_entries > max_entries {
        return Err(AnalysisError::TooManyEntries {
            actual: usize::try_from(advertised_entries).unwrap_or(usize::MAX),
            limit: limits.max_entries,
        });
    }
    // Keep a read-at handle to the immutable snapshot. External model parts
    // can then be inflated and parsed concurrently without reopening the
    // mutable source path or sharing a seek cursor between workers.
    let parallel_snapshot = Arc::new(snapshot.try_clone()?);
    let archive = ZipArchive::new(snapshot)?;
    let mut package = ValidatedPackage::new(archive, byte_size, limits)?;

    validate_content_types(&mut package, limits)?;
    let root_relationships = parse_relationship_entry(
        &mut package,
        ROOT_RELATIONSHIPS_PATH,
        limits.max_relationship_bytes,
        limits,
    )?;
    validate_internal_relationship_targets(&package, &root_relationships)?;
    let root_has_model = root_relationships.iter().any(|relationship| {
        relationship.target_mode.as_deref() != Some("External")
            && relationship
                .relationship_type
                .to_ascii_lowercase()
                .contains("3dmodel")
            && relationship.target == MAIN_MODEL_PATH
    });
    if !root_has_model {
        return Err(AnalysisError::InvalidStructure(format!(
            "{ROOT_RELATIONSHIPS_PATH} does not point to {MAIN_MODEL_PATH}"
        )));
    }

    let mut main = parse_main_model(&mut package, limits)?;
    validate_model_relationships(&mut package, MAIN_MODEL_PATH, &main.external_paths, limits)?;
    validate_remaining_relationships(&mut package, limits)?;

    let mut warnings = std::mem::take(&mut main.warnings);
    let project_settings = if package.contains(PROJECT_SETTINGS_PATH) {
        parse_project_settings(&mut package, limits)?
    } else {
        warnings.push(AnalysisWarning::new(
            WarningCode::MissingProjectSettings,
            "Project settings are absent; printer, process, and declared filament details are limited",
        ));
        ParsedProjectSettings::default()
    };

    let model_settings = if package.contains(MODEL_SETTINGS_PATH) {
        parse_model_settings(&mut package, limits)?
    } else {
        warnings.push(AnalysisWarning::new(
            WarningCode::MissingModelSettings,
            "Model settings are absent; objects and instances are derived from the standard 3MF build",
        ));
        fallback_model_settings(&main)
    };
    warnings.extend(model_settings.warnings.iter().cloned());

    let mut resource_annotations: BTreeMap<(String, u32), BTreeSet<u16>> = BTreeMap::new();
    for (object_id, states) in &main.inline_annotations {
        resource_annotations.insert((MAIN_MODEL_PATH.to_owned(), *object_id), states.clone());
    }

    let mut vertex_count = main.vertex_count;
    let mut triangle_count = main.triangle_count;
    let mut geometry_graph = BTreeMap::new();
    for (object_id, object) in &main.objects {
        geometry_graph.insert(
            (MAIN_MODEL_PATH.to_owned(), *object_id),
            ResourceObject {
                has_mesh: object.has_mesh,
                mesh_bounds: object.mesh_bounds,
                components: object.components.clone(),
            },
        );
    }
    let mut pending_paths = main.external_paths.clone();
    let mut parsed_paths = BTreeSet::from([MAIN_MODEL_PATH.to_owned()]);
    while !pending_paths.is_empty() {
        let mut batch = Vec::new();
        while let Some(external_path) = pending_paths.pop_first() {
            if parsed_paths.insert(external_path.clone()) {
                batch.push((external_path.clone(), package.info(&external_path)?.clone()));
            }
        }

        for (external_path, parsed) in parse_geometry_entries(&parallel_snapshot, batch, limits)? {
            validate_model_relationships(
                &mut package,
                &external_path,
                &parsed.external_paths,
                limits,
            )?;
            vertex_count = vertex_count.saturating_add(parsed.vertex_count);
            triangle_count = triangle_count.saturating_add(parsed.triangle_count);
            for nested_path in &parsed.external_paths {
                if !parsed_paths.contains(nested_path) {
                    pending_paths.insert(nested_path.clone());
                }
            }
            for (object_id, object) in parsed.objects {
                geometry_graph.insert((external_path.clone(), object_id), object);
            }
            for (object_id, states) in parsed.annotations {
                resource_annotations.insert((external_path.clone(), object_id), states);
            }
        }
    }

    detect_dangling_references(&main, &model_settings, &geometry_graph, &mut warnings);

    let (mut filaments, printer, process) =
        build_project_settings(&project_settings, &mut warnings);
    let (objects, plates) = build_objects_and_plates(
        &main,
        &model_settings,
        &resource_annotations,
        &geometry_graph,
        &filaments,
        &mut warnings,
    );

    let mut used_slots = BTreeSet::new();
    for object in &objects {
        used_slots.extend(object.effective_slots.iter().copied());
    }
    for filament in &mut filaments {
        filament.used = used_slots.contains(&filament.slot);
    }
    for slot in used_slots.iter().copied() {
        if usize::from(slot) > filaments.len() {
            warnings.push(AnalysisWarning::new(
                WarningCode::UndeclaredFilamentSlot,
                format!("Source slot {slot} is used but not declared in project settings"),
            ));
        }
    }

    let mut project_roles: BTreeMap<u16, BTreeSet<MaterialRole>> = BTreeMap::new();
    for object in &objects {
        for usage in &object.effective_material_colors {
            for slot in &usage.source_slots {
                project_roles
                    .entry(*slot)
                    .or_default()
                    .extend(usage.roles.iter().copied());
            }
        }
    }
    let effective_material_colors = effective_material_colors(&project_roles, &filaments);

    let source = build_source_information(&main, &project_settings, &package, &mut warnings);
    add_alternative_plate_warning(&plates, &mut warnings);

    let mono_object_count = objects
        .iter()
        .filter(|object| object.classification == ColorClassification::Mono)
        .count();
    let multi_color_object_count = objects
        .iter()
        .filter(|object| object.classification == ColorClassification::MultiColor)
        .count();
    let unassigned_object_count = objects
        .iter()
        .filter(|object| object.classification == ColorClassification::Unassigned)
        .count();
    let summary = AnalysisSummary {
        plate_count: plates.len(),
        object_count: objects.len(),
        instance_count: plates.iter().map(|plate| plate.instances.len()).sum(),
        part_count: objects.iter().map(|object| object.part_count).sum(),
        printable_part_count: objects
            .iter()
            .map(|object| object.printable_part_count)
            .sum(),
        declared_filament_count: filaments.len(),
        used_filament_count: filaments.iter().filter(|filament| filament.used).count(),
        mono_object_count,
        multi_color_object_count,
        unassigned_object_count,
        bounded_printable_part_count: objects
            .iter()
            .flat_map(|object| &object.parts)
            .filter(|part| part.printable && part.object_space_bounds.is_some())
            .count(),
        bounded_object_count: objects
            .iter()
            .filter(|object| object.printable_bounds.is_some())
            .count(),
        bounded_instance_count: plates
            .iter()
            .flat_map(|plate| &plate.instances)
            .filter(|instance| instance.printable_bounds.is_some())
            .count(),
        vertex_count,
        triangle_count,
    };

    let analysis = ProjectAnalysis {
        input: InputIdentity { byte_size, sha256 },
        source,
        archive: package.statistics,
        printer,
        process,
        filaments,
        effective_material_colors,
        plates,
        objects,
        summary,
        warnings,
    };
    source_guard.ensure_unchanged()?;
    Ok(analysis)
}

struct SnapshottedInput {
    snapshot: File,
    identity: InputIdentity,
    source_guard: SourceGuard,
}

struct SourceGuard {
    path: PathBuf,
    source: File,
    fingerprint: SourceFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceFingerprint {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    change_time_seconds: i64,
    #[cfg(unix)]
    change_time_nanoseconds: i64,
}

impl SourceFingerprint {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            change_time_seconds: metadata.ctime(),
            #[cfg(unix)]
            change_time_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

impl SourceGuard {
    fn ensure_unchanged(&self) -> Result<(), AnalysisError> {
        let open_fingerprint = SourceFingerprint::from_metadata(&self.source.metadata()?);
        let path_fingerprint = std::fs::metadata(&self.path)
            .map(|metadata| SourceFingerprint::from_metadata(&metadata))
            .map_err(|_| AnalysisError::SourceChangedDuringAnalysis(self.path.clone()))?;
        if open_fingerprint != self.fingerprint || path_fingerprint != self.fingerprint {
            return Err(AnalysisError::SourceChangedDuringAnalysis(
                self.path.clone(),
            ));
        }
        Ok(())
    }
}

fn snapshot_input(path: &Path, max_bytes: u64) -> Result<SnapshottedInput, AnalysisError> {
    let mut source = File::open(path)?;
    let initial_metadata = source.metadata()?;
    if !initial_metadata.file_type().is_file() {
        return Err(AnalysisError::InputNotRegularFile(path.to_owned()));
    }
    if initial_metadata.len() > max_bytes {
        return Err(AnalysisError::InputArchiveTooLarge {
            actual: initial_metadata.len(),
            limit: max_bytes,
        });
    }
    let fingerprint = SourceFingerprint::from_metadata(&initial_metadata);
    let mut snapshot = tempfile::tempfile()?;
    let identity = copy_and_hash_snapshot(&mut source, &mut snapshot, max_bytes)?;
    snapshot.flush()?;
    snapshot.seek(SeekFrom::Start(0))?;

    let source_guard = SourceGuard {
        path: path.to_owned(),
        source,
        fingerprint,
    };
    source_guard.ensure_unchanged()?;

    Ok(SnapshottedInput {
        snapshot,
        identity,
        source_guard,
    })
}

fn copy_and_hash_snapshot(
    source: &mut File,
    snapshot: &mut File,
    max_bytes: u64,
) -> Result<InputIdentity, AnalysisError> {
    source.seek(SeekFrom::Start(0))?;
    let mut prefix = [0_u8; 4];
    source
        .read_exact(&mut prefix)
        .map_err(|_| AnalysisError::InvalidZipSignature)?;
    if !matches!(
        prefix,
        [b'P', b'K', 3, 4] | [b'P', b'K', 5, 6] | [b'P', b'K', 7, 8]
    ) {
        return Err(AnalysisError::InvalidZipSignature);
    }

    snapshot.write_all(&prefix)?;
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    let mut byte_size = 4_u64;
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        byte_size =
            byte_size
                .checked_add(count as u64)
                .ok_or(AnalysisError::InputArchiveTooLarge {
                    actual: u64::MAX,
                    limit: max_bytes,
                })?;
        if byte_size > max_bytes {
            return Err(AnalysisError::InputArchiveTooLarge {
                actual: byte_size,
                limit: max_bytes,
            });
        }
        snapshot.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
    }
    Ok(InputIdentity {
        byte_size,
        sha256: hex::encode(hasher.finalize()),
    })
}

/// Independent cursor over the immutable analysis snapshot.
///
/// `File::try_clone` shares a seek cursor on Unix, so it cannot safely back
/// concurrent ZIP readers. Positional reads keep each worker's cursor local
/// while all workers still observe the exact bytes that were hashed during
/// snapshot creation.
struct SnapshotReader {
    file: Arc<File>,
    position: u64,
    len: u64,
}

impl SnapshotReader {
    fn new(file: Arc<File>) -> io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(Self {
            file,
            position: 0,
            len,
        })
    }
}

impl Read for SnapshotReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        let read = {
            use std::os::unix::fs::FileExt;
            self.file.read_at(buffer, self.position)?
        };
        #[cfg(windows)]
        let read = {
            use std::os::windows::fs::FileExt;
            self.file.seek_read(buffer, self.position)?
        };
        self.position = self.position.checked_add(read as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "snapshot offset overflow")
        })?;
        Ok(read)
    }
}

impl Seek for SnapshotReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let (base, displacement) = match position {
            SeekFrom::Start(position) => {
                self.position = position;
                return Ok(position);
            }
            SeekFrom::Current(displacement) => (i128::from(self.position), displacement),
            SeekFrom::End(displacement) => (i128::from(self.len), displacement),
        };
        let next = base
            .checked_add(i128::from(displacement))
            .filter(|next| *next >= 0 && *next <= i128::from(u64::MAX))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid snapshot seek"))?;
        self.position = next as u64;
        Ok(self.position)
    }
}

#[derive(Clone, Debug)]
struct EntryInfo {
    index: usize,
    uncompressed_size: u64,
}

struct ValidatedPackage {
    archive: ZipArchive<File>,
    entries: BTreeMap<String, EntryInfo>,
    statistics: ArchiveStatistics,
}

impl ValidatedPackage {
    fn new(
        mut archive: ZipArchive<File>,
        archive_bytes: u64,
        limits: &AnalysisLimits,
    ) -> Result<Self, AnalysisError> {
        if archive.len() > limits.max_entries {
            return Err(AnalysisError::TooManyEntries {
                actual: archive.len(),
                limit: limits.max_entries,
            });
        }

        let mut entries = BTreeMap::new();
        let mut equivalent_names = BTreeMap::<String, String>::new();
        let mut total_compressed_bytes = 0_u64;
        let mut total_uncompressed_bytes = 0_u64;
        let mut largest_entry_uncompressed_bytes = 0_u64;
        let mut maximum_compression_ratio = 1.0_f64;

        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            let name = std::str::from_utf8(entry.name_raw())
                .map_err(|_| AnalysisError::NonUtf8EntryName)?
                .to_owned();
            validate_entry_name(&name, entry.is_dir(), limits)?;
            if entry.encrypted() {
                return Err(AnalysisError::EncryptedEntry(name));
            }
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(AnalysisError::SymbolicLinkEntry(name));
            }
            let entry_size_limit = entry_uncompressed_size_limit(&name, limits);
            if entry.size() > entry_size_limit {
                return Err(AnalysisError::EntryTooLarge {
                    path: name,
                    actual: entry.size(),
                    limit: entry_size_limit,
                });
            }

            total_compressed_bytes = total_compressed_bytes
                .checked_add(entry.compressed_size())
                .ok_or(AnalysisError::ArchiveTooLarge {
                    actual: u64::MAX,
                    limit: limits.max_total_uncompressed_bytes,
                })?;
            total_uncompressed_bytes = total_uncompressed_bytes.checked_add(entry.size()).ok_or(
                AnalysisError::ArchiveTooLarge {
                    actual: u64::MAX,
                    limit: limits.max_total_uncompressed_bytes,
                },
            )?;
            if total_uncompressed_bytes > limits.max_total_uncompressed_bytes {
                return Err(AnalysisError::ArchiveTooLarge {
                    actual: total_uncompressed_bytes,
                    limit: limits.max_total_uncompressed_bytes,
                });
            }

            let ratio = match (entry.size(), entry.compressed_size()) {
                (0, _) => 1.0,
                (_, 0) => f64::INFINITY,
                (size, compressed) => size as f64 / compressed as f64,
            };
            if ratio > limits.max_compression_ratio {
                return Err(AnalysisError::CompressionRatioExceeded {
                    path: name,
                    actual: ratio,
                    limit: limits.max_compression_ratio,
                });
            }
            largest_entry_uncompressed_bytes = largest_entry_uncompressed_bytes.max(entry.size());
            maximum_compression_ratio = maximum_compression_ratio.max(ratio);
            let info = EntryInfo {
                index,
                uncompressed_size: entry.size(),
            };
            if entries.insert(name.clone(), info).is_some() {
                return Err(AnalysisError::DuplicateEntry(name));
            }
            if !entry.is_dir() {
                let equivalent = name.to_ascii_lowercase();
                if let Some(first) = equivalent_names.insert(equivalent, name.clone()) {
                    return Err(AnalysisError::EquivalentEntryName {
                        first,
                        second: name,
                    });
                }
            }
        }

        for (comparison_name, original_name) in &equivalent_names {
            for (separator, _) in comparison_name.match_indices('/') {
                let ancestor = &comparison_name[..separator];
                if let Some(ancestor_name) = equivalent_names.get(ancestor) {
                    return Err(AnalysisError::PartNameDerivationConflict {
                        first: ancestor_name.clone(),
                        second: original_name.clone(),
                    });
                }
            }
        }

        let entry_count = entries.len();
        Ok(Self {
            archive,
            entries,
            statistics: ArchiveStatistics {
                entry_count,
                archive_bytes,
                total_compressed_bytes,
                total_uncompressed_bytes,
                largest_entry_uncompressed_bytes,
                maximum_compression_ratio,
            },
        })
    }

    fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    fn info(&self, path: &str) -> Result<&EntryInfo, AnalysisError> {
        self.entries
            .get(path)
            .ok_or_else(|| AnalysisError::MissingRequiredEntry(path.to_owned()))
    }

    fn read_bytes(&mut self, path: &str, limit: u64) -> Result<Vec<u8>, AnalysisError> {
        let info = self.info(path)?.clone();
        if info.uncompressed_size > limit {
            return Err(AnalysisError::MetadataEntryTooLarge {
                path: path.to_owned(),
                limit,
            });
        }
        let capacity = usize::try_from(info.uncompressed_size).map_err(|_| {
            AnalysisError::MetadataEntryTooLarge {
                path: path.to_owned(),
                limit,
            }
        })?;
        let mut data = Vec::with_capacity(capacity);
        self.archive.by_index(info.index)?.read_to_end(&mut data)?;
        Ok(data)
    }
}

fn entry_uncompressed_size_limit(path: &str, limits: &AnalysisLimits) -> u64 {
    if is_model_xml_entry(path) {
        limits.max_model_entry_uncompressed_bytes
    } else {
        limits.max_entry_uncompressed_bytes
    }
}

fn is_model_xml_entry(path: &str) -> bool {
    path.starts_with("3D/")
        && path
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("model"))
}

fn validate_entry_name(
    name: &str,
    is_directory: bool,
    limits: &AnalysisLimits,
) -> Result<(), AnalysisError> {
    let unsafe_path = |reason: &str| AnalysisError::UnsafeEntryPath {
        path: name.to_owned(),
        reason: reason.to_owned(),
    };
    if name.is_empty() {
        return Err(unsafe_path("empty path"));
    }
    if name.len() > limits.max_entry_name_bytes {
        return Err(unsafe_path("path exceeds the configured byte limit"));
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err(unsafe_path("absolute path"));
    }
    if name.contains('\\') {
        return Err(unsafe_path("backslash path separator"));
    }
    if name.contains('\0')
        || name.contains('?')
        || name.contains('#')
        || name.contains('%')
        || name.chars().any(char::is_control)
    {
        return Err(unsafe_path(
            "control character, query, fragment, or percent escape",
        ));
    }

    let segments: Vec<_> = name.split('/').collect();
    for (index, segment) in segments.iter().enumerate() {
        if segment.is_empty() && !(is_directory && index + 1 == segments.len()) {
            return Err(unsafe_path("empty path segment"));
        }
        if matches!(*segment, "." | "..") {
            return Err(unsafe_path("dot traversal segment"));
        }
        if segment.contains(':') {
            return Err(unsafe_path(
                "colon is not allowed in a portable package path",
            ));
        }
    }
    Ok(())
}

fn validate_content_types(
    package: &mut ValidatedPackage,
    limits: &AnalysisLimits,
) -> Result<(), AnalysisError> {
    let data = package.read_bytes(CONTENT_TYPES_PATH, limits.max_relationship_bytes)?;
    let mut reader = Reader::from_reader(BufReader::new(XmlTokenLimitedReader::new(
        data.as_slice(),
        limits.max_xml_token_bytes,
    )));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut has_types = false;
    let mut has_model_content_type = false;
    let mut depth = 0_usize;
    loop {
        match read_xml_event(&mut reader, &mut buffer, CONTENT_TYPES_PATH)? {
            Event::Start(event) => {
                depth = checked_depth(depth, limits, CONTENT_TYPES_PATH)?;
                if local_eq(event.name().as_ref(), b"Types") {
                    has_types = true;
                }
                if local_eq(event.name().as_ref(), b"Default")
                    || local_eq(event.name().as_ref(), b"Override")
                {
                    has_model_content_type |= content_type_is_model(&reader, &event)?;
                }
            }
            Event::Empty(event) => {
                if local_eq(event.name().as_ref(), b"Default")
                    || local_eq(event.name().as_ref(), b"Override")
                {
                    has_model_content_type |= content_type_is_model(&reader, &event)?;
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::DocType(_) => {
                return Err(AnalysisError::ForbiddenDoctype(CONTENT_TYPES_PATH.into()));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !has_types || !has_model_content_type {
        return Err(AnalysisError::InvalidStructure(
            "[Content_Types].xml does not declare a 3MF model content type".into(),
        ));
    }
    Ok(())
}

fn content_type_is_model<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
) -> Result<bool, AnalysisError> {
    Ok(xml_attr(reader, event, b"ContentType", CONTENT_TYPES_PATH)?
        .is_some_and(|value| value.to_ascii_lowercase().contains("3dmodel")))
}

#[derive(Clone, Debug)]
struct Relationship {
    target: String,
    relationship_type: String,
    target_mode: Option<String>,
}

fn parse_relationship_entry(
    package: &mut ValidatedPackage,
    path: &str,
    size_limit: u64,
    limits: &AnalysisLimits,
) -> Result<Vec<Relationship>, AnalysisError> {
    let data = package.read_bytes(path, size_limit)?;
    let mut reader = Reader::from_reader(BufReader::new(XmlTokenLimitedReader::new(
        data.as_slice(),
        limits.max_xml_token_bytes,
    )));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut relationships = Vec::new();
    let mut ids = HashSet::new();
    let mut has_root = false;
    let mut depth = 0_usize;
    loop {
        match read_xml_event(&mut reader, &mut buffer, path)? {
            Event::Start(event) => {
                depth = checked_depth(depth, limits, path)?;
                if local_eq(event.name().as_ref(), b"Relationships") {
                    has_root = true;
                }
                if local_eq(event.name().as_ref(), b"Relationship") {
                    relationships.push(parse_relationship(&reader, &event, path, &mut ids)?);
                }
            }
            Event::Empty(event) if local_eq(event.name().as_ref(), b"Relationship") => {
                relationships.push(parse_relationship(&reader, &event, path, &mut ids)?);
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::DocType(_) => return Err(AnalysisError::ForbiddenDoctype(path.into())),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !has_root {
        return Err(xml_error(path, "missing Relationships root element"));
    }
    Ok(relationships)
}

fn parse_relationship<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    source_path: &str,
    ids: &mut HashSet<String>,
) -> Result<Relationship, AnalysisError> {
    let id = required_attr(reader, event, b"Id", source_path)?;
    if !ids.insert(id.clone()) {
        return Err(AnalysisError::InvalidStructure(format!(
            "duplicate relationship ID {id:?} in {source_path}"
        )));
    }
    let raw_target = required_attr(reader, event, b"Target", source_path)?;
    let relationship_type = required_attr(reader, event, b"Type", source_path)?;
    let target_mode = xml_attr(reader, event, b"TargetMode", source_path)?;
    let target = if target_mode.as_deref() == Some("External") {
        raw_target
    } else {
        let source_part = source_path_for_relationship(source_path)?;
        resolve_package_target(&source_part, &raw_target)?
    };
    Ok(Relationship {
        target,
        relationship_type,
        target_mode,
    })
}

fn source_path_for_relationship(relationship_path: &str) -> Result<String, AnalysisError> {
    if relationship_path == ROOT_RELATIONSHIPS_PATH {
        return Ok(String::new());
    }

    let marker = "_rels/";
    let marker_index = relationship_path.rfind(marker).ok_or_else(|| {
        AnalysisError::InvalidStructure(format!(
            "relationship part {relationship_path:?} is not inside an _rels directory"
        ))
    })?;
    if marker_index > 0 && relationship_path.as_bytes()[marker_index - 1] != b'/' {
        return Err(AnalysisError::InvalidStructure(format!(
            "relationship part {relationship_path:?} is not inside an _rels directory"
        )));
    }
    let relationship_name = &relationship_path[marker_index + marker.len()..];
    let source_name = relationship_name.strip_suffix(".rels").ok_or_else(|| {
        AnalysisError::InvalidStructure(format!(
            "relationship part {relationship_path:?} does not end in .rels"
        ))
    })?;
    if source_name.is_empty() {
        return Err(AnalysisError::InvalidStructure(format!(
            "relationship part {relationship_path:?} has no source part"
        )));
    }
    Ok(format!(
        "{}{source_name}",
        &relationship_path[..marker_index]
    ))
}

fn validate_model_relationships(
    package: &mut ValidatedPackage,
    source_path: &str,
    external_paths: &BTreeSet<String>,
    limits: &AnalysisLimits,
) -> Result<(), AnalysisError> {
    let relationships_path = relationship_path_for_part(source_path);
    if !package.contains(&relationships_path) {
        return if external_paths.is_empty() {
            Ok(())
        } else {
            Err(AnalysisError::MissingRequiredEntry(relationships_path))
        };
    }
    let relationships = parse_relationship_entry(
        package,
        &relationships_path,
        limits.max_relationship_bytes,
        limits,
    )?;
    validate_internal_relationship_targets(package, &relationships)?;
    let targets: HashSet<_> = relationships
        .into_iter()
        .filter(|relationship| relationship.target_mode.as_deref() != Some("External"))
        .filter(|relationship| {
            relationship
                .relationship_type
                .to_ascii_lowercase()
                .contains("3dmodel")
        })
        .map(|relationship| relationship.target)
        .collect();
    for target in external_paths {
        if !package.contains(target) {
            return Err(AnalysisError::MissingReferencedEntry(target.clone()));
        }
        if !targets.contains(target) {
            return Err(AnalysisError::InvalidStructure(format!(
                "external model {target:?} is not declared by {relationships_path}"
            )));
        }
    }
    Ok(())
}

fn relationship_path_for_part(source_path: &str) -> String {
    let (directory, file_name) = source_path
        .rsplit_once('/')
        .map_or(("", source_path), |(directory, file_name)| {
            (directory, file_name)
        });
    if directory.is_empty() {
        format!("_rels/{file_name}.rels")
    } else {
        format!("{directory}/_rels/{file_name}.rels")
    }
}

fn validate_remaining_relationships(
    package: &mut ValidatedPackage,
    limits: &AnalysisLimits,
) -> Result<(), AnalysisError> {
    let paths: Vec<_> = package
        .entries
        .keys()
        .filter(|path| path.ends_with(".rels"))
        .filter(|path| {
            path.as_str() != ROOT_RELATIONSHIPS_PATH && path.as_str() != MAIN_RELATIONSHIPS_PATH
        })
        .cloned()
        .collect();
    for path in paths {
        let relationships =
            parse_relationship_entry(package, &path, limits.max_relationship_bytes, limits)?;
        validate_internal_relationship_targets(package, &relationships)?;
    }
    Ok(())
}

fn validate_internal_relationship_targets(
    package: &ValidatedPackage,
    relationships: &[Relationship],
) -> Result<(), AnalysisError> {
    for relationship in relationships {
        if relationship.target_mode.as_deref() != Some("External")
            && !package.contains(&relationship.target)
        {
            return Err(AnalysisError::MissingReferencedEntry(
                relationship.target.clone(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct MainModel {
    metadata: BTreeMap<String, String>,
    objects: BTreeMap<u32, MainObject>,
    build_items: Vec<BuildItem>,
    inline_annotations: BTreeMap<u32, BTreeSet<u16>>,
    external_paths: BTreeSet<String>,
    has_bambu_namespace: bool,
    unit_scale_mm: f64,
    vertex_count: u64,
    triangle_count: u64,
    warnings: Vec<AnalysisWarning>,
}

impl Default for MainModel {
    fn default() -> Self {
        Self {
            metadata: BTreeMap::new(),
            objects: BTreeMap::new(),
            build_items: Vec::new(),
            inline_annotations: BTreeMap::new(),
            external_paths: BTreeSet::new(),
            has_bambu_namespace: false,
            unit_scale_mm: 1.0,
            vertex_count: 0,
            triangle_count: 0,
            warnings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct MainObject {
    name: Option<String>,
    has_mesh: bool,
    mesh_bounds: Option<AxisAlignedBounds>,
    components: Vec<ComponentReference>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ResourceObject {
    has_mesh: bool,
    mesh_bounds: Option<AxisAlignedBounds>,
    components: Vec<ComponentReference>,
}

#[derive(Clone, Debug, PartialEq)]
struct ComponentReference {
    object_id: u32,
    path: Option<String>,
    transform: Option<Transform3mf>,
}

#[derive(Clone, Debug)]
struct BuildItem {
    source_index: u32,
    object_id: u32,
    path: Option<String>,
    printable: bool,
    transform: Option<Transform3mf>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct ResourceObjectKey {
    path: String,
    object_id: u32,
}

impl ResourceObjectKey {
    fn new(path: impl Into<String>, object_id: u32) -> Self {
        Self {
            path: path.into(),
            object_id,
        }
    }
}

impl BuildItem {
    fn resource_key(&self) -> ResourceObjectKey {
        ResourceObjectKey::new(
            self.path.as_deref().unwrap_or(MAIN_MODEL_PATH),
            self.object_id,
        )
    }
}

fn parse_main_model(
    package: &mut ValidatedPackage,
    limits: &AnalysisLimits,
) -> Result<MainModel, AnalysisError> {
    let info = package.info(MAIN_MODEL_PATH)?.clone();
    let entry = package.archive.by_index(info.index)?;
    let mut reader = Reader::from_reader(BufReader::with_capacity(
        MODEL_XML_BUFFER_BYTES,
        XmlTokenLimitedReader::new(entry, limits.max_xml_token_bytes),
    ));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut model = MainModel::default();
    let mut current_object: Option<u32> = None;
    let mut pending_metadata: Option<String> = None;
    let mut metadata_text = String::new();
    let mut depth = 0_usize;
    let mut uuids = HashSet::new();

    loop {
        match read_xml_event(&mut reader, &mut buffer, MAIN_MODEL_PATH)? {
            Event::Start(event) => {
                depth = checked_depth(depth, limits, MAIN_MODEL_PATH)?;
                handle_main_element(
                    &reader,
                    &event,
                    false,
                    &mut model,
                    &mut current_object,
                    &mut pending_metadata,
                    &mut metadata_text,
                    &mut uuids,
                    limits,
                )?;
            }
            Event::Empty(event) => {
                handle_main_element(
                    &reader,
                    &event,
                    true,
                    &mut model,
                    &mut current_object,
                    &mut pending_metadata,
                    &mut metadata_text,
                    &mut uuids,
                    limits,
                )?;
            }
            Event::Text(event) if pending_metadata.is_some() => {
                let decoded = event
                    .xml_content()
                    .map_err(|error| xml_error(MAIN_MODEL_PATH, error))?;
                let unescaped = quick_xml::escape::unescape(&decoded)
                    .map_err(|error| xml_error(MAIN_MODEL_PATH, error))?;
                metadata_text.push_str(&unescaped);
            }
            Event::CData(event) if pending_metadata.is_some() => {
                metadata_text.push_str(
                    &event
                        .decode()
                        .map_err(|error| xml_error(MAIN_MODEL_PATH, error))?,
                );
            }
            Event::End(event) => {
                if local_eq(event.name().as_ref(), b"metadata") {
                    if let Some(name) = pending_metadata.take() {
                        model.metadata.insert(name, metadata_text.trim().to_owned());
                    }
                    metadata_text.clear();
                } else if local_eq(event.name().as_ref(), b"object") {
                    current_object = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::DocType(_) => {
                return Err(AnalysisError::ForbiddenDoctype(MAIN_MODEL_PATH.into()));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(model)
}

#[allow(clippy::too_many_arguments)]
fn handle_main_element<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    empty: bool,
    model: &mut MainModel,
    current_object: &mut Option<u32>,
    pending_metadata: &mut Option<String>,
    metadata_text: &mut String,
    uuids: &mut HashSet<String>,
    limits: &AnalysisLimits,
) -> Result<(), AnalysisError> {
    let name = event.name();
    if local_eq(name.as_ref(), b"model") {
        for attribute in event.attributes() {
            let attribute = attribute.map_err(|error| xml_error(MAIN_MODEL_PATH, error))?;
            let attribute_name = local_name(attribute.key.as_ref());
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| xml_error(MAIN_MODEL_PATH, error))?;
            if attribute_name == b"unit" {
                model.unit_scale_mm = unit_scale_mm(&value, MAIN_MODEL_PATH)?;
            }
            if value.contains("schemas.bambulab.com") || value.contains("orcaslicer") {
                model.has_bambu_namespace = true;
            }
        }
    } else if local_eq(name.as_ref(), b"metadata") {
        if let Some(metadata_name) = xml_attr(reader, event, b"name", MAIN_MODEL_PATH)?
            && matches!(
                metadata_name.as_str(),
                "Application" | "Title" | "BambuStudio:3mfVersion" | "OrcaSlicer:3mfVersion"
            )
        {
            if empty {
                model.metadata.insert(metadata_name, String::new());
            } else {
                *pending_metadata = Some(metadata_name);
                metadata_text.clear();
            }
        }
    } else if local_eq(name.as_ref(), b"object") {
        let id = required_u32_attr(reader, event, b"id", MAIN_MODEL_PATH)?;
        if model.objects.contains_key(&id) {
            return Err(AnalysisError::InvalidStructure(format!(
                "duplicate object ID {id} in {MAIN_MODEL_PATH}"
            )));
        }
        if let Some(uuid) = xml_attr(reader, event, b"UUID", MAIN_MODEL_PATH)?
            && !uuids.insert(uuid.clone())
        {
            model.warnings.push(
                AnalysisWarning::new(
                    WarningCode::DuplicateUuid,
                    format!("Duplicate resource UUID {uuid}"),
                )
                .for_object(id),
            );
        }
        let object = MainObject {
            name: xml_attr(reader, event, b"name", MAIN_MODEL_PATH)?,
            has_mesh: false,
            mesh_bounds: None,
            components: Vec::new(),
        };
        model.objects.insert(id, object);
        if !empty {
            *current_object = Some(id);
        }
    } else if local_eq(name.as_ref(), b"mesh") {
        let owner = current_object.ok_or_else(|| {
            AnalysisError::InvalidStructure("mesh outside a resource object".into())
        })?;
        model
            .objects
            .get_mut(&owner)
            .expect("current object exists")
            .has_mesh = true;
    } else if local_eq(name.as_ref(), b"component") {
        let owner = current_object.ok_or_else(|| {
            AnalysisError::InvalidStructure("component outside a resource object".into())
        })?;
        let object_id = required_u32_attr(reader, event, b"objectid", MAIN_MODEL_PATH)?;
        let path = xml_attr(reader, event, b"path", MAIN_MODEL_PATH)?
            .map(|path| resolve_package_target(MAIN_MODEL_PATH, &path))
            .transpose()?;
        if let Some(path) = &path {
            model.external_paths.insert(path.clone());
        }
        let transform = xml_attr(reader, event, b"transform", MAIN_MODEL_PATH)?
            .map(|value| parse_transform(&value, MAIN_MODEL_PATH, model.unit_scale_mm))
            .transpose()?;
        model
            .objects
            .get_mut(&owner)
            .expect("current object exists")
            .components
            .push(ComponentReference {
                object_id,
                path,
                transform,
            });
    } else if local_eq(name.as_ref(), b"item") {
        let object_id = required_u32_attr(reader, event, b"objectid", MAIN_MODEL_PATH)?;
        let path = xml_attr(reader, event, b"path", MAIN_MODEL_PATH)?
            .map(|path| resolve_package_target(MAIN_MODEL_PATH, &path))
            .transpose()?;
        if let Some(path) = &path {
            model.external_paths.insert(path.clone());
        }
        let printable = xml_attr(reader, event, b"printable", MAIN_MODEL_PATH)?
            .map(|value| parse_bool(&value))
            .transpose()
            .map_err(AnalysisError::InvalidStructure)?
            .unwrap_or(true);
        let transform = xml_attr(reader, event, b"transform", MAIN_MODEL_PATH)?
            .map(|value| parse_transform(&value, MAIN_MODEL_PATH, model.unit_scale_mm))
            .transpose()?;
        let source_index = u32::try_from(model.build_items.len()).map_err(|_| {
            AnalysisError::InvalidStructure("source model has too many build items".into())
        })?;
        model.build_items.push(BuildItem {
            source_index,
            object_id,
            path,
            printable,
            transform,
        });
    } else if local_eq(name.as_ref(), b"vertex") {
        model.vertex_count = model.vertex_count.saturating_add(1);
        let owner = current_object.ok_or_else(|| {
            AnalysisError::InvalidStructure("vertex outside a resource object".into())
        })?;
        let point = parse_vertex(reader, event, MAIN_MODEL_PATH, model.unit_scale_mm)?;
        extend_bounds(
            &mut model
                .objects
                .get_mut(&owner)
                .expect("current object exists")
                .mesh_bounds,
            point,
        )?;
    } else if local_eq(name.as_ref(), b"triangle") {
        model.triangle_count = model.triangle_count.saturating_add(1);
        if let Some(object_id) = *current_object {
            let states = paint_states_from_attributes(
                reader,
                event,
                MAIN_MODEL_PATH,
                limits.max_paint_annotation_chars,
            )?;
            model
                .inline_annotations
                .entry(object_id)
                .or_default()
                .extend(states);
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct ParsedGeometry {
    annotations: BTreeMap<u32, BTreeSet<u16>>,
    objects: BTreeMap<u32, ResourceObject>,
    external_paths: BTreeSet<String>,
    unit_scale_mm: f64,
    vertex_count: u64,
    triangle_count: u64,
}

impl Default for ParsedGeometry {
    fn default() -> Self {
        Self {
            annotations: BTreeMap::new(),
            objects: BTreeMap::new(),
            external_paths: BTreeSet::new(),
            unit_scale_mm: 1.0,
            vertex_count: 0,
            triangle_count: 0,
        }
    }
}

fn parse_geometry_reader<R: Read>(
    entry: R,
    path: &str,
    limits: &AnalysisLimits,
) -> Result<ParsedGeometry, AnalysisError> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(
        MODEL_XML_BUFFER_BYTES,
        XmlTokenLimitedReader::new(entry, limits.max_xml_token_bytes),
    ));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut parsed = ParsedGeometry::default();
    let mut current_object = None;
    let mut depth = 0_usize;
    loop {
        match read_xml_event(&mut reader, &mut buffer, path)? {
            Event::Start(event) => {
                depth = checked_depth(depth, limits, path)?;
                handle_geometry_element(
                    &reader,
                    &event,
                    path,
                    false,
                    &mut current_object,
                    &mut parsed,
                    limits,
                )?;
            }
            Event::Empty(event) => {
                handle_geometry_element(
                    &reader,
                    &event,
                    path,
                    true,
                    &mut current_object,
                    &mut parsed,
                    limits,
                )?;
            }
            Event::End(event) => {
                if local_eq(event.name().as_ref(), b"object") {
                    current_object = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::DocType(_) => return Err(AnalysisError::ForbiddenDoctype(path.into())),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(parsed)
}

fn parse_geometry_entries(
    snapshot: &Arc<File>,
    entries: Vec<(String, EntryInfo)>,
    limits: &AnalysisLimits,
) -> Result<Vec<(String, ParsedGeometry)>, AnalysisError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    // Eight workers are enough to saturate decompression/XML parsing on the
    // desktop while keeping the bounded per-worker buffers modest. A single
    // CPU or a single external part naturally stays on one worker.
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(8)
        .min(entries.len());
    parse_geometry_entries_with_worker_count(snapshot, entries, limits, worker_count)
}

fn parse_geometry_entries_with_worker_count(
    snapshot: &Arc<File>,
    entries: Vec<(String, EntryInfo)>,
    limits: &AnalysisLimits,
    worker_count: usize,
) -> Result<Vec<(String, ParsedGeometry)>, AnalysisError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = worker_count.max(1).min(entries.len());
    let chunk_size = entries.len().div_ceil(worker_count);
    let mut workers = Vec::with_capacity(worker_count);
    for chunk in entries.chunks(chunk_size) {
        let snapshot = Arc::clone(snapshot);
        let chunk = chunk.to_vec();
        let limits = limits.clone();
        workers.push(std::thread::spawn(move || {
            let reader = SnapshotReader::new(snapshot)?;
            let mut archive = ZipArchive::new(reader)?;
            let mut parsed_entries = Vec::with_capacity(chunk.len());
            for (path, info) in chunk {
                let entry = archive.by_index(info.index)?;
                let parsed = parse_geometry_reader(entry, &path, &limits)?;
                parsed_entries.push((path, parsed));
            }
            Ok::<_, AnalysisError>(parsed_entries)
        }));
    }

    let mut parsed_entries = Vec::with_capacity(entries.len());
    let mut first_error = None;
    for worker in workers {
        match worker.join() {
            Ok(Ok(mut parsed)) => parsed_entries.append(&mut parsed),
            Ok(Err(error)) if first_error.is_none() => first_error = Some(error),
            Err(_) if first_error.is_none() => {
                first_error = Some(AnalysisError::InvalidStructure(
                    "external model analysis worker failed".into(),
                ));
            }
            Ok(Err(_)) | Err(_) => {}
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    // Chunking is contiguous, but keep the deterministic path order explicit
    // so future scheduling changes cannot affect warnings or error context.
    parsed_entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(parsed_entries)
}

#[allow(clippy::too_many_arguments)]
fn handle_geometry_element<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    empty: bool,
    current_object: &mut Option<u32>,
    parsed: &mut ParsedGeometry,
    limits: &AnalysisLimits,
) -> Result<(), AnalysisError> {
    let name = event.name();
    if local_eq(name.as_ref(), b"model") {
        if let Some(unit) = xml_attr(reader, event, b"unit", path)? {
            parsed.unit_scale_mm = unit_scale_mm(&unit, path)?;
        }
    } else if local_eq(name.as_ref(), b"object") {
        let id = required_u32_attr(reader, event, b"id", path)?;
        if parsed
            .objects
            .insert(id, ResourceObject::default())
            .is_some()
        {
            return Err(AnalysisError::InvalidStructure(format!(
                "duplicate object ID {id} in {path}"
            )));
        }
        if !empty {
            *current_object = Some(id);
        }
    } else if local_eq(name.as_ref(), b"mesh") {
        let owner = current_object.ok_or_else(|| {
            AnalysisError::InvalidStructure(format!("mesh outside an object in {path}"))
        })?;
        parsed
            .objects
            .get_mut(&owner)
            .expect("current object exists")
            .has_mesh = true;
    } else if local_eq(name.as_ref(), b"component") {
        let owner = current_object.ok_or_else(|| {
            AnalysisError::InvalidStructure(format!("component outside an object in {path}"))
        })?;
        let object_id = required_u32_attr(reader, event, b"objectid", path)?;
        let component_path = xml_attr(reader, event, b"path", path)?
            .map(|target| resolve_package_target(path, &target))
            .transpose()?;
        if let Some(component_path) = &component_path {
            parsed.external_paths.insert(component_path.clone());
        }
        let transform = xml_attr(reader, event, b"transform", path)?
            .map(|value| parse_transform(&value, path, parsed.unit_scale_mm))
            .transpose()?;
        parsed
            .objects
            .get_mut(&owner)
            .expect("current object exists")
            .components
            .push(ComponentReference {
                object_id,
                path: component_path,
                transform,
            });
    } else if local_eq(name.as_ref(), b"vertex") {
        parsed.vertex_count = parsed.vertex_count.saturating_add(1);
        let owner = current_object.ok_or_else(|| {
            AnalysisError::InvalidStructure(format!("vertex outside an object in {path}"))
        })?;
        let point = parse_vertex(reader, event, path, parsed.unit_scale_mm)?;
        extend_bounds(
            &mut parsed
                .objects
                .get_mut(&owner)
                .expect("current object exists")
                .mesh_bounds,
            point,
        )?;
    } else if local_eq(name.as_ref(), b"triangle") {
        parsed.triangle_count = parsed.triangle_count.saturating_add(1);
        let object_id = current_object.ok_or_else(|| {
            AnalysisError::InvalidStructure(format!("triangle outside an object in {path}"))
        })?;
        let states =
            paint_states_from_attributes(reader, event, path, limits.max_paint_annotation_chars)?;
        parsed
            .annotations
            .entry(object_id)
            .or_default()
            .extend(states);
    }
    Ok(())
}

fn paint_states_from_attributes<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    max_chars: usize,
) -> Result<BTreeSet<u16>, AnalysisError> {
    let mut states = BTreeSet::new();
    let mut attributes = event.attributes();
    attributes.with_checks(false);
    let mut seen = SeenAttributeNames::new();
    for attribute in attributes {
        let attribute = attribute.map_err(|error| xml_error(path, error))?;
        if !seen.insert(attribute.key) {
            return Err(xml_error(
                path,
                format!(
                    "duplicated attribute {}",
                    String::from_utf8_lossy(attribute.key.as_ref())
                ),
            ));
        }
        let attribute_name = local_name(attribute.key.as_ref());
        if attribute_name != b"paint_color" && attribute_name != b"mmu_segmentation" {
            continue;
        }
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| xml_error(path, error))?;
        if value.len() > max_chars {
            return Err(AnalysisError::InvalidStructure(format!(
                "paint annotation in {path} exceeds {max_chars} characters"
            )));
        }
        for state in
            used_paint_states(&value).map_err(|source| AnalysisError::InvalidPaintAnnotation {
                entry: path.to_owned(),
                source,
            })?
        {
            if state != 0 {
                states.insert(u16::from(state));
            }
        }
    }
    Ok(states)
}

#[derive(Clone, Debug, Default)]
struct ParsedProjectSettings {
    root: Option<Value>,
}

fn parse_project_settings(
    package: &mut ValidatedPackage,
    limits: &AnalysisLimits,
) -> Result<ParsedProjectSettings, AnalysisError> {
    let data = package.read_bytes(PROJECT_SETTINGS_PATH, limits.max_config_bytes)?;
    let root = serde_json::from_slice(&data).map_err(|error| AnalysisError::InvalidJson {
        entry: PROJECT_SETTINGS_PATH.into(),
        message: error.to_string(),
    })?;
    Ok(ParsedProjectSettings { root: Some(root) })
}

#[derive(Clone, Debug, Default)]
struct ParsedModelSettings {
    objects: Vec<SettingsObject>,
    plates: Vec<SettingsPlate>,
    warnings: Vec<AnalysisWarning>,
}

#[derive(Clone, Debug)]
struct SettingsObject {
    id: u32,
    resource_path: Option<String>,
    name: Option<String>,
    extruder: Option<u16>,
    support_slots: BTreeMap<u16, MaterialRole>,
    parts: Vec<SettingsPart>,
}

#[derive(Clone, Debug)]
struct SettingsPart {
    id: u32,
    name: Option<String>,
    subtype: String,
    extruder: Option<u16>,
}

#[derive(Clone, Debug, Default)]
struct SettingsPlate {
    id: Option<u32>,
    name: Option<String>,
    instances: Vec<SettingsInstance>,
}

#[derive(Clone, Debug, Default)]
struct SettingsInstance {
    object_id: Option<u32>,
    resource_path: Option<String>,
    instance_id: Option<u32>,
    identify_id: Option<u64>,
}

fn parse_model_settings(
    package: &mut ValidatedPackage,
    limits: &AnalysisLimits,
) -> Result<ParsedModelSettings, AnalysisError> {
    let info = package.info(MODEL_SETTINGS_PATH)?.clone();
    if info.uncompressed_size > limits.max_config_bytes {
        return Err(AnalysisError::MetadataEntryTooLarge {
            path: MODEL_SETTINGS_PATH.into(),
            limit: limits.max_config_bytes,
        });
    }
    let entry = package.archive.by_index(info.index)?;
    let mut reader = Reader::from_reader(BufReader::with_capacity(
        MODEL_XML_BUFFER_BYTES,
        XmlTokenLimitedReader::new(entry, limits.max_xml_token_bytes),
    ));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut parsed = ParsedModelSettings::default();
    let mut current_object: Option<SettingsObject> = None;
    let mut current_part: Option<SettingsPart> = None;
    let mut current_plate: Option<SettingsPlate> = None;
    let mut current_instance: Option<SettingsInstance> = None;
    let mut object_ids = HashSet::new();
    let mut depth = 0_usize;

    loop {
        match read_xml_event(&mut reader, &mut buffer, MODEL_SETTINGS_PATH)? {
            Event::Start(event) => {
                depth = checked_depth(depth, limits, MODEL_SETTINGS_PATH)?;
                if local_eq(event.name().as_ref(), b"object") && current_plate.is_none() {
                    let id = required_u32_attr(&reader, &event, b"id", MODEL_SETTINGS_PATH)?;
                    if !object_ids.insert(id) {
                        return Err(AnalysisError::InvalidStructure(format!(
                            "duplicate object ID {id} in {MODEL_SETTINGS_PATH}"
                        )));
                    }
                    current_object = Some(SettingsObject {
                        id,
                        resource_path: None,
                        name: None,
                        extruder: None,
                        support_slots: BTreeMap::new(),
                        parts: Vec::new(),
                    });
                } else if local_eq(event.name().as_ref(), b"part") {
                    current_part = Some(SettingsPart {
                        id: required_u32_attr(&reader, &event, b"id", MODEL_SETTINGS_PATH)?,
                        name: None,
                        subtype: xml_attr(&reader, &event, b"subtype", MODEL_SETTINGS_PATH)?
                            .unwrap_or_else(|| "unknown".into()),
                        extruder: None,
                    });
                } else if local_eq(event.name().as_ref(), b"plate") {
                    current_plate = Some(SettingsPlate::default());
                } else if local_eq(event.name().as_ref(), b"model_instance") {
                    current_instance = Some(SettingsInstance::default());
                } else if local_eq(event.name().as_ref(), b"metadata") {
                    apply_settings_metadata(
                        &reader,
                        &event,
                        &mut current_object,
                        &mut current_part,
                        &mut current_plate,
                        &mut current_instance,
                    )?;
                }
            }
            Event::Empty(event) if local_eq(event.name().as_ref(), b"metadata") => {
                apply_settings_metadata(
                    &reader,
                    &event,
                    &mut current_object,
                    &mut current_part,
                    &mut current_plate,
                    &mut current_instance,
                )?;
            }
            Event::End(event) => {
                if local_eq(event.name().as_ref(), b"part") {
                    let part = current_part.take().ok_or_else(|| {
                        AnalysisError::InvalidStructure("part close without open part".into())
                    })?;
                    current_object
                        .as_mut()
                        .ok_or_else(|| {
                            AnalysisError::InvalidStructure("part outside object".into())
                        })?
                        .parts
                        .push(part);
                } else if local_eq(event.name().as_ref(), b"object") {
                    let object = current_object.take().ok_or_else(|| {
                        AnalysisError::InvalidStructure("object close without open object".into())
                    })?;
                    parsed.objects.push(object);
                } else if local_eq(event.name().as_ref(), b"model_instance") {
                    let instance = current_instance.take().ok_or_else(|| {
                        AnalysisError::InvalidStructure(
                            "model_instance close without open instance".into(),
                        )
                    })?;
                    current_plate
                        .as_mut()
                        .ok_or_else(|| {
                            AnalysisError::InvalidStructure("model_instance outside plate".into())
                        })?
                        .instances
                        .push(instance);
                } else if local_eq(event.name().as_ref(), b"plate") {
                    let plate = current_plate.take().ok_or_else(|| {
                        AnalysisError::InvalidStructure("plate close without open plate".into())
                    })?;
                    parsed.plates.push(plate);
                }
                depth = depth.saturating_sub(1);
            }
            Event::DocType(_) => {
                return Err(AnalysisError::ForbiddenDoctype(MODEL_SETTINGS_PATH.into()));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    Ok(parsed)
}

fn apply_settings_metadata<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    current_object: &mut Option<SettingsObject>,
    current_part: &mut Option<SettingsPart>,
    current_plate: &mut Option<SettingsPlate>,
    current_instance: &mut Option<SettingsInstance>,
) -> Result<(), AnalysisError> {
    let Some(key) = xml_attr(reader, event, b"key", MODEL_SETTINGS_PATH)? else {
        return Ok(());
    };
    let Some(value) = xml_attr(reader, event, b"value", MODEL_SETTINGS_PATH)? else {
        return Ok(());
    };
    if let Some(instance) = current_instance.as_mut() {
        match key.as_str() {
            "object_id" => instance.object_id = Some(parse_u32(&value, MODEL_SETTINGS_PATH)?),
            "instance_id" => instance.instance_id = Some(parse_u32(&value, MODEL_SETTINGS_PATH)?),
            "identify_id" => {
                instance.identify_id = Some(value.parse().map_err(|_| {
                    AnalysisError::InvalidStructure(format!(
                        "invalid identify_id {value:?} in {MODEL_SETTINGS_PATH}"
                    ))
                })?)
            }
            _ => {}
        }
    } else if let Some(plate) = current_plate.as_mut() {
        match key.as_str() {
            "plater_id" => plate.id = Some(parse_u32(&value, MODEL_SETTINGS_PATH)?),
            "plater_name" => plate.name = nonempty(value),
            _ => {}
        }
    } else if let Some(part) = current_part.as_mut() {
        match key.as_str() {
            "name" => part.name = nonempty(value),
            "extruder" => part.extruder = parse_slot(&value),
            _ => {}
        }
    } else if let Some(object) = current_object.as_mut() {
        match key.as_str() {
            "name" => object.name = nonempty(value),
            "extruder" => object.extruder = parse_slot(&value),
            "support_filament" => {
                if let Some(slot) = parse_slot(&value) {
                    object.support_slots.insert(slot, MaterialRole::Support);
                }
            }
            "support_interface_filament" => {
                if let Some(slot) = parse_slot(&value) {
                    object
                        .support_slots
                        .insert(slot, MaterialRole::SupportInterface);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn fallback_model_settings(main: &MainModel) -> ParsedModelSettings {
    let object_keys: BTreeSet<_> = main
        .build_items
        .iter()
        .map(BuildItem::resource_key)
        .collect();
    let objects = object_keys
        .iter()
        .map(|key| {
            let main_object = (key.path == MAIN_MODEL_PATH)
                .then(|| main.objects.get(&key.object_id))
                .flatten();
            SettingsObject {
                id: key.object_id,
                resource_path: Some(key.path.clone()),
                name: main_object.and_then(|object| object.name.clone()),
                extruder: None,
                support_slots: BTreeMap::new(),
                parts: main_object.map_or_else(Vec::new, |object| {
                    object
                        .components
                        .iter()
                        .map(|component| SettingsPart {
                            id: component.object_id,
                            name: None,
                            subtype: "normal_part".into(),
                            extruder: None,
                        })
                        .collect()
                }),
            }
        })
        .collect();
    let plates = if main.build_items.is_empty() {
        Vec::new()
    } else {
        let mut next_instance_id = BTreeMap::<ResourceObjectKey, u32>::new();
        let instances = main
            .build_items
            .iter()
            .map(|item| {
                let key = item.resource_key();
                let instance_id = next_instance_id.entry(key.clone()).or_default();
                let instance = SettingsInstance {
                    object_id: Some(item.object_id),
                    resource_path: Some(key.path),
                    instance_id: Some(*instance_id),
                    identify_id: None,
                };
                *instance_id = instance_id.saturating_add(1);
                instance
            })
            .collect();
        vec![SettingsPlate {
            id: Some(1),
            name: Some("Build plate".into()),
            instances,
        }]
    };
    ParsedModelSettings {
        objects,
        plates,
        warnings: Vec::new(),
    }
}

fn settings_resource_key(
    main: &MainModel,
    object_id: u32,
    resource_path: Option<&str>,
) -> Result<ResourceObjectKey, ()> {
    if let Some(resource_path) = resource_path {
        return Ok(ResourceObjectKey::new(resource_path, object_id));
    }
    let mut selected: Option<ResourceObjectKey> = None;
    for item in main
        .build_items
        .iter()
        .filter(|item| item.object_id == object_id)
    {
        let key = item.resource_key();
        if selected.as_ref().is_some_and(|selected| selected != &key) {
            return Err(());
        }
        selected = Some(key);
    }
    Ok(selected.unwrap_or_else(|| ResourceObjectKey::new(MAIN_MODEL_PATH, object_id)))
}

fn analyzed_object_ids(
    main: &MainModel,
    settings: &ParsedModelSettings,
) -> BTreeMap<ResourceObjectKey, u32> {
    let mut keys: BTreeSet<_> = main
        .build_items
        .iter()
        .map(BuildItem::resource_key)
        .collect();
    for object in &settings.objects {
        if let Ok(key) = settings_resource_key(main, object.id, object.resource_path.as_deref()) {
            keys.insert(key);
        }
    }

    let mut used_ids: BTreeSet<_> = keys.iter().map(|key| key.object_id).collect();
    let mut first_key_by_source_id = BTreeSet::new();
    let mut next_surrogate = 1_u32;
    keys.into_iter()
        .map(|key| {
            let analyzed_id = if first_key_by_source_id.insert(key.object_id) {
                key.object_id
            } else {
                loop {
                    if used_ids.insert(next_surrogate) {
                        break next_surrogate;
                    }
                    next_surrogate = next_surrogate.wrapping_add(1).max(1);
                }
            };
            (key, analyzed_id)
        })
        .collect()
}

fn build_project_settings(
    parsed: &ParsedProjectSettings,
    warnings: &mut Vec<AnalysisWarning>,
) -> (
    Vec<DeclaredFilament>,
    PrinterInformation,
    ProcessInformation,
) {
    let Some(root) = parsed.root.as_ref() else {
        return (
            Vec::new(),
            PrinterInformation::default(),
            ProcessInformation::default(),
        );
    };
    let colors = positional_string_array(root.get("filament_colour"));
    let materials = positional_string_array(root.get("filament_type"));
    let presets = positional_string_array(root.get("filament_settings_id"));
    let requested_count = colors.len().max(materials.len()).max(presets.len());
    let count = requested_count.min(usize::from(u16::MAX));
    if requested_count > count {
        warnings.push(AnalysisWarning::new(
            WarningCode::InvalidSetting,
            format!(
                "Ignoring filament settings beyond the maximum logical slot {}",
                u16::MAX
            ),
        ));
    }
    if count > 0 && (colors.len() != count || materials.len() != count || presets.len() != count) {
        warnings.push(AnalysisWarning::new(
            WarningCode::MismatchedFilamentArrays,
            format!(
                "Filament setting arrays have different lengths (colors {}, materials {}, presets {})",
                colors.len(),
                materials.len(),
                presets.len()
            ),
        ));
    }
    let filaments = (0..count)
        .map(|index| {
            let color = colors.get(index).cloned().flatten().map(|color| {
                let normalized = color.to_ascii_uppercase();
                if !valid_color(&normalized) {
                    warnings.push(AnalysisWarning::new(
                        WarningCode::InvalidColor,
                        format!("Filament slot {} has invalid color {color:?}", index + 1),
                    ));
                }
                normalized
            });
            DeclaredFilament {
                slot: (index + 1) as u16,
                material: materials.get(index).cloned().flatten().and_then(nonempty),
                color: color.and_then(nonempty),
                preset: presets.get(index).cloned().flatten().and_then(nonempty),
                used: false,
            }
        })
        .collect();

    let nozzle_diameters_mm = positional_string_array(root.get("nozzle_diameter"))
        .into_iter()
        .flatten()
        .filter_map(|value| match parse_positive_f64(&value) {
            Some(value) => Some(value),
            None => {
                warnings.push(AnalysisWarning::new(
                    WarningCode::InvalidSetting,
                    format!("Ignoring invalid nozzle diameter {value:?}"),
                ));
                None
            }
        })
        .collect();
    let printer = PrinterInformation {
        model: string_value(root.get("printer_model")),
        variant: string_value(root.get("printer_variant")),
        nozzle_diameters_mm,
    };
    let process = ProcessInformation {
        name: string_value(root.get("print_settings_id")),
        layer_height_mm: numeric_setting(root, "layer_height", warnings),
        initial_layer_height_mm: numeric_setting(root, "initial_layer_print_height", warnings),
        prime_tower_enabled: bool_value(root.get("enable_prime_tower")),
        quality: build_quality_information(root, warnings),
        support: build_support_information(root, warnings),
    };
    (filaments, printer, process)
}

fn build_quality_information(
    root: &Value,
    warnings: &mut Vec<AnalysisWarning>,
) -> QualityInformation {
    let wall_generator =
        string_value(root.get("wall_generator")).and_then(|value| match value.as_str() {
            "classic" => Some(WallGenerator::Classic),
            "arachne" => Some(WallGenerator::Arachne),
            _ => {
                warnings.push(AnalysisWarning::new(
                    WarningCode::InvalidSetting,
                    format!("Ignoring unsupported wall_generator value {value:?}"),
                ));
                None
            }
        });
    QualityInformation {
        wall_generator,
        outer_wall_speed_mm_s: minimum_numeric_setting(root, "outer_wall_speed", warnings),
        inner_wall_speed_mm_s: minimum_numeric_setting(root, "inner_wall_speed", warnings),
        top_surface_speed_mm_s: minimum_numeric_setting(root, "top_surface_speed", warnings),
        outer_wall_acceleration_mm_s2: minimum_numeric_setting(
            root,
            "outer_wall_acceleration",
            warnings,
        ),
        wall_loops: integer_setting(root, "wall_loops", warnings),
        top_shell_layers: integer_setting(root, "top_shell_layers", warnings),
        bottom_shell_layers: integer_setting(root, "bottom_shell_layers", warnings),
    }
}

fn build_support_information(
    root: &Value,
    warnings: &mut Vec<AnalysisWarning>,
) -> SupportInformation {
    let enabled = boolean_setting(root, "enable_support", warnings);
    let on_build_plate_only = boolean_setting(root, "support_on_build_plate_only", warnings);
    let support_type =
        string_value(root.get("support_type")).and_then(|value| match value.as_str() {
            "normal(auto)" => Some(SupportType::NormalAuto),
            "tree(auto)" => Some(SupportType::TreeAuto),
            _ => {
                warnings.push(AnalysisWarning::new(
                    WarningCode::InvalidSetting,
                    format!("Ignoring unsupported support_type value {value:?}"),
                ));
                None
            }
        });
    let threshold_angle_degrees =
        string_value(root.get("support_threshold_angle")).and_then(|raw| {
            let parsed = raw.parse::<f64>().ok();
            match parsed {
                Some(value)
                    if value.is_finite()
                        && (0.0..=90.0).contains(&value)
                        && value.fract().abs() <= f64::EPSILON =>
                {
                    Some(value as u8)
                }
                _ => {
                    warnings.push(AnalysisWarning::new(
                        WarningCode::InvalidSetting,
                        format!("Ignoring invalid support_threshold_angle value {raw:?}"),
                    ));
                    None
                }
            }
        });
    SupportInformation {
        enabled,
        support_type,
        threshold_angle_degrees,
        on_build_plate_only,
    }
}

fn boolean_setting(root: &Value, key: &str, warnings: &mut Vec<AnalysisWarning>) -> Option<bool> {
    let value = root.get(key)?;
    match bool_value(Some(value)) {
        Some(value) => Some(value),
        None => {
            warnings.push(AnalysisWarning::new(
                WarningCode::InvalidSetting,
                format!("Ignoring invalid {key} value {value}"),
            ));
            None
        }
    }
}

fn detect_dangling_references(
    main: &MainModel,
    settings: &ParsedModelSettings,
    geometry_graph: &BTreeMap<(String, u32), ResourceObject>,
    warnings: &mut Vec<AnalysisWarning>,
) {
    for ((source_path, owner_id), object) in geometry_graph {
        for component in &object.components {
            let path = component.path.as_deref().unwrap_or(source_path);
            let target_exists =
                geometry_graph.contains_key(&(path.to_owned(), component.object_id));
            if !target_exists {
                let mut warning = AnalysisWarning::new(
                    WarningCode::UnsafeBuildInstanceGraph,
                    format!(
                        "Object {} references missing resource object {} in {path}",
                        owner_id, component.object_id
                    ),
                )
                .for_object(*owner_id);
                warning.entry_path = Some(path.to_owned());
                warnings.push(warning);
            }
        }
    }

    for item in &main.build_items {
        let path = item.path.as_deref().unwrap_or(MAIN_MODEL_PATH);
        if !geometry_graph.contains_key(&(path.to_owned(), item.object_id)) {
            let mut warning = AnalysisWarning::new(
                WarningCode::UnsafeBuildInstanceGraph,
                format!(
                    "Build item references missing object {} in {path}",
                    item.object_id
                ),
            )
            .for_object(item.object_id);
            warning.entry_path = Some(path.to_owned());
            warnings.push(warning);
        }
    }

    for object in &settings.objects {
        let Ok(key) = settings_resource_key(main, object.id, object.resource_path.as_deref())
        else {
            warnings.push(
                AnalysisWarning::new(
                    WarningCode::UnsafeBuildInstanceGraph,
                    format!(
                        "Object {} is referenced by build items from multiple model parts",
                        object.id
                    ),
                )
                .for_object(object.id),
            );
            continue;
        };
        if !geometry_graph.contains_key(&(key.path.clone(), key.object_id)) {
            let mut warning = AnalysisWarning::new(
                WarningCode::UnsafeBuildInstanceGraph,
                format!(
                    "Model settings reference object {} absent from {}",
                    object.id, key.path,
                ),
            )
            .for_object(object.id);
            warning.entry_path = Some(key.path);
            warnings.push(warning);
        }
    }
}

fn numeric_setting(root: &Value, key: &str, warnings: &mut Vec<AnalysisWarning>) -> Option<f64> {
    let raw = string_value(root.get(key))?;
    match parse_positive_f64(&raw) {
        Some(value) => Some(value),
        None => {
            warnings.push(AnalysisWarning::new(
                WarningCode::InvalidSetting,
                format!("Ignoring invalid {key} value {raw:?}"),
            ));
            None
        }
    }
}

fn minimum_numeric_setting(
    root: &Value,
    key: &str,
    warnings: &mut Vec<AnalysisWarning>,
) -> Option<f64> {
    let raw_values = positional_string_array(root.get(key));
    if raw_values.is_empty() {
        return None;
    }
    let parsed = raw_values
        .iter()
        .map(|raw| raw.as_deref().and_then(parse_positive_f64))
        .collect::<Option<Vec<_>>>();
    let Some(parsed) = parsed else {
        warnings.push(AnalysisWarning::new(
            WarningCode::InvalidSetting,
            format!("Ignoring invalid {key} value"),
        ));
        return None;
    };
    parsed.into_iter().reduce(f64::min)
}

fn integer_setting(root: &Value, key: &str, warnings: &mut Vec<AnalysisWarning>) -> Option<u16> {
    let raw = string_value(root.get(key))?;
    match raw.trim().parse::<u16>().ok().filter(|value| *value > 0) {
        Some(value) => Some(value),
        None => {
            warnings.push(AnalysisWarning::new(
                WarningCode::InvalidSetting,
                format!("Ignoring invalid {key} value {raw:?}"),
            ));
            None
        }
    }
}

fn compose_transforms(inner: Transform3mf, outer: Transform3mf) -> Option<Transform3mf> {
    if !inner.is_finite() || !outer.is_finite() {
        return None;
    }
    let mut values = [0.0; 12];
    for row in 0..3 {
        for column in 0..3 {
            values[column * 3 + row] = (0..3)
                .map(|axis| outer.values[axis * 3 + row] * inner.values[column * 3 + axis])
                .sum();
        }
        values[9 + row] = (0..3)
            .map(|axis| outer.values[axis * 3 + row] * inner.values[9 + axis])
            .sum::<f64>()
            + outer.values[9 + row];
    }
    let transform = Transform3mf { values };
    transform.is_finite().then_some(transform)
}

fn resolve_resource_bounds(
    geometry_graph: &BTreeMap<(String, u32), ResourceObject>,
    path: &str,
    object_id: u32,
    transform: Transform3mf,
) -> Option<AxisAlignedBounds> {
    resolve_resource_bounds_inner(
        geometry_graph,
        path,
        object_id,
        transform,
        0,
        &mut BTreeSet::new(),
    )
}

fn resolve_resource_bounds_inner(
    geometry_graph: &BTreeMap<(String, u32), ResourceObject>,
    path: &str,
    object_id: u32,
    transform: Transform3mf,
    depth: usize,
    visiting: &mut BTreeSet<(String, u32)>,
) -> Option<AxisAlignedBounds> {
    if depth > MAX_COMPONENT_GRAPH_DEPTH {
        return None;
    }
    let key = (path.to_owned(), object_id);
    if !visiting.insert(key.clone()) {
        return None;
    }
    let result = (|| {
        let object = geometry_graph.get(&key)?;
        let mut bounds = None;
        let mut has_geometry = false;
        if object.has_mesh {
            has_geometry = true;
            let mesh_bounds = object.mesh_bounds?.transformed(transform)?;
            bounds = Some(mesh_bounds);
        }
        for component in &object.components {
            has_geometry = true;
            let child_path = component.path.as_deref().unwrap_or(path);
            let child_transform = compose_transforms(
                component.transform.unwrap_or(Transform3mf::IDENTITY),
                transform,
            )?;
            let child_bounds = resolve_resource_bounds_inner(
                geometry_graph,
                child_path,
                component.object_id,
                child_transform,
                depth + 1,
                visiting,
            )?;
            bounds = Some(match bounds {
                Some(existing) => existing.union(child_bounds)?,
                None => child_bounds,
            });
        }
        has_geometry.then_some(bounds).flatten()
    })();
    visiting.remove(&key);
    result
}

fn resource_paint_states(
    geometry_graph: &BTreeMap<(String, u32), ResourceObject>,
    annotations: &BTreeMap<(String, u32), BTreeSet<u16>>,
    path: &str,
    object_id: u32,
) -> BTreeSet<u16> {
    let mut states = BTreeSet::new();
    collect_resource_paint_states(
        geometry_graph,
        annotations,
        path,
        object_id,
        0,
        &mut BTreeSet::new(),
        &mut states,
    );
    states
}

fn collect_resource_paint_states(
    geometry_graph: &BTreeMap<(String, u32), ResourceObject>,
    annotations: &BTreeMap<(String, u32), BTreeSet<u16>>,
    path: &str,
    object_id: u32,
    depth: usize,
    visiting: &mut BTreeSet<(String, u32)>,
    states: &mut BTreeSet<u16>,
) {
    if depth > MAX_COMPONENT_GRAPH_DEPTH {
        return;
    }
    let key = (path.to_owned(), object_id);
    if !visiting.insert(key.clone()) {
        return;
    }
    if let Some(direct) = annotations.get(&key) {
        states.extend(direct.iter().copied());
    }
    if let Some(object) = geometry_graph.get(&key) {
        for component in &object.components {
            let child_path = component.path.as_deref().unwrap_or(path);
            collect_resource_paint_states(
                geometry_graph,
                annotations,
                child_path,
                component.object_id,
                depth + 1,
                visiting,
                states,
            );
        }
    }
    visiting.remove(&key);
}

fn complete_bounds(
    bounds: impl IntoIterator<Item = Option<AxisAlignedBounds>>,
) -> Option<AxisAlignedBounds> {
    let mut combined: Option<AxisAlignedBounds> = None;
    let mut count = 0_usize;
    for bounds in bounds {
        count += 1;
        let bounds = bounds?;
        combined = Some(match combined {
            Some(existing) => existing.union(bounds)?,
            None => bounds,
        });
    }
    (count > 0).then_some(combined).flatten()
}

fn transformed_object_bounds(
    object: &ObjectAnalysis,
    transform: Transform3mf,
) -> Option<AxisAlignedBounds> {
    let printable_parts = object.parts.iter().filter(|part| part.printable);
    if object.printable_part_count == 0 {
        object.printable_bounds?.transformed(transform)
    } else {
        complete_bounds(
            printable_parts.map(|part| part.object_space_bounds?.transformed(transform)),
        )
    }
}

fn complete_instance_build_mappings(
    main: &MainModel,
    settings: &ParsedModelSettings,
    build_by_object: &HashMap<ResourceObjectKey, Vec<&BuildItem>>,
    warnings: &mut Vec<AnalysisWarning>,
) -> HashSet<ResourceObjectKey> {
    let mut settings_ids_by_object = HashMap::<ResourceObjectKey, Vec<Option<u32>>>::new();
    for instance in settings
        .plates
        .iter()
        .flat_map(|plate| plate.instances.iter())
    {
        if let Some(object_id) = instance.object_id {
            let Ok(key) = settings_resource_key(main, object_id, instance.resource_path.as_deref())
            else {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnsafeBuildInstanceGraph,
                        format!(
                            "Object {object_id} instance cannot be matched uniquely because the resource ID occurs in multiple model parts"
                        ),
                    )
                    .for_object(object_id),
                );
                continue;
            };
            settings_ids_by_object
                .entry(key)
                .or_default()
                .push(instance.instance_id);
        }
    }

    let mut complete = HashSet::new();
    for (key, instance_ids) in settings_ids_by_object {
        let Some(build_items) = build_by_object.get(&key) else {
            warnings.push(
                AnalysisWarning::new(
                    WarningCode::UnsafeBuildInstanceGraph,
                    format!(
                        "Object {} instances have no matching 3MF build items in {}; affected instances are excluded from planning",
                        key.object_id, key.path,
                    ),
                )
                .for_object(key.object_id),
            );
            continue;
        };

        // Bambu/Orca instance_id is the zero-based index within this object's
        // build items. Validate the complete index set before trusting any
        // transform so malformed metadata cannot silently select another copy.
        let unique_single = build_items.len() == 1 && instance_ids.len() == 1;
        let mut seen = HashSet::with_capacity(instance_ids.len());
        let dense_index_set = instance_ids.len() == build_items.len()
            && instance_ids.iter().all(|instance_id| {
                instance_id.is_some_and(|instance_id| {
                    usize::try_from(instance_id)
                        .is_ok_and(|index| index < build_items.len() && seen.insert(instance_id))
                })
            });
        if unique_single || dense_index_set {
            complete.insert(key);
        } else {
            warnings.push(
                AnalysisWarning::new(
                    WarningCode::UnsafeBuildInstanceGraph,
                    format!(
                        "Object {} instance IDs do not map uniquely to its 3MF build items in {}; affected instances are excluded from planning",
                        key.object_id, key.path,
                    ),
                )
                .for_object(key.object_id),
            );
        }
    }
    complete
}

fn build_objects_and_plates(
    main: &MainModel,
    settings: &ParsedModelSettings,
    annotations: &BTreeMap<(String, u32), BTreeSet<u16>>,
    geometry_graph: &BTreeMap<(String, u32), ResourceObject>,
    filaments: &[DeclaredFilament],
    warnings: &mut Vec<AnalysisWarning>,
) -> (Vec<ObjectAnalysis>, Vec<PlateAnalysis>) {
    let analyzed_ids = analyzed_object_ids(main, settings);
    let mut objects = Vec::with_capacity(settings.objects.len());
    for source in &settings.objects {
        let source_key =
            settings_resource_key(main, source.id, source.resource_path.as_deref()).ok();
        let source_resource_path = source_key.as_ref().map(|key| key.path.as_str());
        let analyzed_id = source_key
            .as_ref()
            .and_then(|key| analyzed_ids.get(key))
            .copied()
            .unwrap_or(source.id);
        let main_object = source_key
            .as_ref()
            .filter(|key| key.path == MAIN_MODEL_PATH)
            .and_then(|_| main.objects.get(&source.id));
        let default_resource_path = source_resource_path.unwrap_or(MAIN_MODEL_PATH);
        let mut roles: BTreeMap<u16, BTreeSet<MaterialRole>> = BTreeMap::new();
        let mut parts = Vec::with_capacity(source.parts.len());

        for source_part in &source.parts {
            let volume_type = parse_volume_type(&source_part.subtype);
            if let VolumeType::Unknown(raw) = &volume_type {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnknownVolumeType,
                        format!("Unknown volume subtype {raw:?}"),
                    )
                    .for_object(source.id),
                );
            }
            let printable = volume_type.is_printable_positive();
            let component = main_object.and_then(|object| {
                object
                    .components
                    .iter()
                    .find(|component| component.object_id == source_part.id)
            });
            let component_path = component
                .and_then(|component| component.path.clone())
                .or_else(|| Some(default_resource_path.to_owned()));
            let (resource_path, resource_object_id, component_transform) = component.map_or(
                (
                    default_resource_path,
                    source_part.id,
                    Transform3mf::IDENTITY,
                ),
                |component| {
                    (
                        component.path.as_deref().unwrap_or(default_resource_path),
                        component.object_id,
                        component.transform.unwrap_or(Transform3mf::IDENTITY),
                    )
                },
            );
            let painted_slots: Vec<u16> = resource_paint_states(
                geometry_graph,
                annotations,
                resource_path,
                resource_object_id,
            )
            .into_iter()
            .collect();
            let object_space_bounds = resolve_resource_bounds(
                geometry_graph,
                resource_path,
                resource_object_id,
                component_transform,
            );
            let effective_base = source_part.extruder.or(source.extruder);
            let mut effective_slots = BTreeSet::new();
            if printable {
                if let Some(slot) = effective_base {
                    effective_slots.insert(slot);
                }
                effective_slots.extend(painted_slots.iter().copied());
                if effective_slots.is_empty() {
                    warnings.push(
                        AnalysisWarning::new(
                            WarningCode::MissingColorAssignment,
                            format!(
                                "Printable part {} has no object, part, or facet color assignment",
                                source_part.id
                            ),
                        )
                        .for_object(source.id),
                    );
                }
                for slot in effective_slots.iter().copied() {
                    roles.entry(slot).or_default().insert(MaterialRole::Model);
                }
            }
            parts.push(PartAnalysis {
                id: source_part.id,
                name: source_part.name.clone(),
                volume_type,
                printable,
                extruder_slot: source_part.extruder,
                inherited_extruder_slot: source_part
                    .extruder
                    .is_none()
                    .then_some(source.extruder)
                    .flatten(),
                painted_slots,
                effective_slots: effective_slots.into_iter().collect(),
                component_path,
                component_transform: component.and_then(|component| component.transform),
                object_space_bounds,
            });
        }

        if source.parts.is_empty() {
            if let Some(slot) = source.extruder {
                roles.entry(slot).or_default().insert(MaterialRole::Model);
            }
            if let Some(resource_path) = source_resource_path {
                for slot in
                    resource_paint_states(geometry_graph, annotations, resource_path, source.id)
                {
                    roles.entry(slot).or_default().insert(MaterialRole::Model);
                }
            }
            if roles.is_empty() {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::MissingColorAssignment,
                        "Printable object has no object or facet color assignment",
                    )
                    .for_object(source.id),
                );
            }
        }
        for (slot, role) in &source.support_slots {
            roles.entry(*slot).or_default().insert(*role);
        }

        let effective_slots: Vec<_> = roles.keys().copied().collect();
        let material_colors = effective_material_colors(&roles, filaments);
        let classification = classify(material_colors.len());
        let printable_part_count = parts.iter().filter(|part| part.printable).count();
        let printable_bounds = if source.parts.is_empty() {
            source_resource_path.and_then(|resource_path| {
                resolve_resource_bounds(
                    geometry_graph,
                    resource_path,
                    source.id,
                    Transform3mf::IDENTITY,
                )
            })
        } else {
            complete_bounds(
                parts
                    .iter()
                    .filter(|part| part.printable)
                    .map(|part| part.object_space_bounds),
            )
        };
        if (printable_part_count > 0 || source.parts.is_empty()) && printable_bounds.is_none() {
            warnings.push(
                AnalysisWarning::new(
                    WarningCode::UnknownGeometryBounds,
                    "Printable geometry bounds are unavailable; A1 routing is disabled",
                )
                .for_object(source.id),
            );
        }
        objects.push(ObjectAnalysis {
            id: analyzed_id,
            source_model_path: source_key
                .as_ref()
                .filter(|key| key.path != MAIN_MODEL_PATH)
                .map(|key| key.path.clone()),
            source_object_id: (analyzed_id != source.id).then_some(source.id),
            name: source
                .name
                .clone()
                .or_else(|| main_object.and_then(|object| object.name.clone())),
            part_count: parts.len(),
            printable_part_count,
            parts,
            instance_count: 0,
            plate_ids: Vec::new(),
            object_extruder_slot: source.extruder,
            effective_slots,
            effective_material_colors: material_colors,
            classification,
            printable_bounds,
        });
    }

    let object_indices: HashMap<_, _> = objects
        .iter()
        .enumerate()
        .map(|(index, object)| (object.id, index))
        .collect();
    let mut build_by_object: HashMap<ResourceObjectKey, Vec<&BuildItem>> = HashMap::new();
    for item in &main.build_items {
        build_by_object
            .entry(item.resource_key())
            .or_default()
            .push(item);
    }
    let complete_build_mappings =
        complete_instance_build_mappings(main, settings, &build_by_object, warnings);
    let mut membership = HashMap::new();
    let mut referenced_build_items = HashMap::<u32, (u32, u32, u32)>::new();
    let mut plates = Vec::with_capacity(settings.plates.len());

    for (plate_index, source_plate) in settings.plates.iter().enumerate() {
        let plate_id = source_plate.id.unwrap_or((plate_index + 1) as u32);
        let mut instances = Vec::with_capacity(source_plate.instances.len());
        let mut roles: BTreeMap<u16, BTreeSet<MaterialRole>> = BTreeMap::new();
        let mut part_count = 0_usize;
        for source_instance in &source_plate.instances {
            let Some(object_id) = source_instance.object_id else {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnsafeBuildInstanceGraph,
                        "Plate instance has no object_id",
                    )
                    .for_plate(plate_id),
                );
                continue;
            };
            let Ok(resource_key) =
                settings_resource_key(main, object_id, source_instance.resource_path.as_deref())
            else {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnsafeBuildInstanceGraph,
                        format!(
                            "Plate instance references ambiguous object {object_id} from multiple model parts"
                        ),
                    )
                    .for_object(object_id)
                    .for_plate(plate_id),
                );
                continue;
            };
            let Some(&analyzed_id) = analyzed_ids.get(&resource_key) else {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnsafeBuildInstanceGraph,
                        format!(
                            "Plate references unknown object {object_id} in {}",
                            resource_key.path,
                        ),
                    )
                    .for_object(object_id)
                    .for_plate(plate_id),
                );
                continue;
            };
            let instance_id = source_instance.instance_id.unwrap_or(0);
            if let Some(previous_plate) = membership.insert((analyzed_id, instance_id), plate_id) {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnsafeBuildInstanceGraph,
                        format!(
                            "Object {analyzed_id} instance {instance_id} is duplicated in source plate metadata (plates {previous_plate} and {plate_id})"
                        ),
                    )
                    .for_object(analyzed_id)
                    .for_plate(plate_id),
                );
            }
            let Some(&object_index) = object_indices.get(&analyzed_id) else {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnsafeBuildInstanceGraph,
                        format!("Plate references unknown object {analyzed_id}"),
                    )
                    .for_object(analyzed_id)
                    .for_plate(plate_id),
                );
                continue;
            };
            let object = &mut objects[object_index];
            object.instance_count += 1;
            if !object.plate_ids.contains(&plate_id) {
                object.plate_ids.push(plate_id);
            }
            part_count = part_count.saturating_add(object.part_count);
            for usage in &object.effective_material_colors {
                for slot in &usage.source_slots {
                    roles
                        .entry(*slot)
                        .or_default()
                        .extend(usage.roles.iter().copied());
                }
            }
            let build = complete_build_mappings
                .contains(&resource_key)
                .then(|| build_by_object.get(&resource_key))
                .flatten()
                .and_then(|items| {
                    if items.len() == 1 {
                        items.first().copied()
                    } else {
                        usize::try_from(instance_id)
                            .ok()
                            .and_then(|id| items.get(id).copied())
                    }
                });
            let has_printable_geometry = object.printable_part_count > 0 || object.part_count == 0;
            if let Some(build) = build
                && let Some((previous_plate, previous_object, previous_instance)) =
                    referenced_build_items
                        .insert(build.source_index, (plate_id, analyzed_id, instance_id))
            {
                warnings.push(
                    AnalysisWarning::new(
                        WarningCode::UnsafeBuildInstanceGraph,
                        format!(
                            "Source build item {} is referenced more than once: object {previous_object} instance {previous_instance} on plate {previous_plate}, and object {analyzed_id} instance {instance_id} on plate {plate_id}",
                            build.source_index
                        ),
                    )
                    .for_object(analyzed_id)
                    .for_plate(plate_id),
                );
            }
            let printable = has_printable_geometry && build.is_some_and(|item| item.printable);
            let transform = build.and_then(|item| item.transform);
            let printable_bounds = if printable && build.is_some() {
                transformed_object_bounds(object, transform.unwrap_or(Transform3mf::IDENTITY))
            } else {
                None
            };
            instances.push(ObjectInstance {
                object_id: analyzed_id,
                instance_id,
                source_build_item_index: build.map(|item| item.source_index),
                identify_id: source_instance.identify_id,
                printable,
                transform,
                printable_bounds,
            });
        }
        let effective_material_colors = effective_material_colors(&roles, filaments);
        let printable_bounds = complete_bounds(
            instances
                .iter()
                .filter(|instance| instance.printable)
                .map(|instance| instance.printable_bounds),
        );
        plates.push(PlateAnalysis {
            id: plate_id,
            name: source_plate.name.clone(),
            printable_bounds,
            object_count: instances.len(),
            part_count,
            effective_slots: roles.keys().copied().collect(),
            classification: classify(effective_material_colors.len()),
            effective_material_colors,
            instances,
        });
    }

    for build in main.build_items.iter().filter(|build| {
        build.printable && !referenced_build_items.contains_key(&build.source_index)
    }) {
        warnings.push(
            AnalysisWarning::new(
                WarningCode::UnsafeBuildInstanceGraph,
                format!(
                    "Printable source build item {} for object {} in {} is not referenced exactly once by plate metadata",
                    build.source_index,
                    build.object_id,
                    build.path.as_deref().unwrap_or(MAIN_MODEL_PATH),
                ),
            )
            .for_object(build.object_id),
        );
    }

    for object in &objects {
        if object.instance_count == 0 {
            warnings.push(
                AnalysisWarning::new(
                    WarningCode::UnassignedInstance,
                    format!("Object {} is not assigned to a source plate", object.id),
                )
                .for_object(object.id),
            );
        }
    }
    (objects, plates)
}

fn effective_material_colors(
    roles_by_slot: &BTreeMap<u16, BTreeSet<MaterialRole>>,
    filaments: &[DeclaredFilament],
) -> Vec<EffectiveMaterialColor> {
    type Key = (
        Option<String>,
        Option<String>,
        Vec<MaterialRole>,
        Option<String>,
        Option<u16>,
    );
    let mut grouped: BTreeMap<Key, (BTreeSet<u16>, BTreeSet<MaterialRole>)> = BTreeMap::new();
    for (slot, roles) in roles_by_slot {
        let filament = slot
            .checked_sub(1)
            .and_then(|index| filaments.get(usize::from(index)));
        let material = filament.and_then(|filament| filament.material.clone());
        let color = filament.and_then(|filament| filament.color.clone());
        let source_profile_id = filament.and_then(|filament| filament.preset.clone());
        let discriminator =
            (material.is_none() || color.is_none() || source_profile_id.is_none()).then_some(*slot);
        let role_identity = roles.iter().copied().collect::<Vec<_>>();
        let value = grouped
            .entry((
                material,
                color,
                role_identity,
                source_profile_id,
                discriminator,
            ))
            .or_default();
        value.0.insert(*slot);
        value.1.extend(roles.iter().copied());
    }
    grouped
        .into_iter()
        .map(
            |((material, color, _, source_profile_id, _), (slots, roles))| EffectiveMaterialColor {
                source_slots: slots.into_iter().collect(),
                source_profile_ids: source_profile_id.into_iter().collect(),
                material,
                color,
                roles: roles.into_iter().collect(),
            },
        )
        .collect()
}

fn classify(effective_color_count: usize) -> ColorClassification {
    match effective_color_count {
        0 => ColorClassification::Unassigned,
        1 => ColorClassification::Mono,
        _ => ColorClassification::MultiColor,
    }
}

fn build_source_information(
    main: &MainModel,
    project_settings: &ParsedProjectSettings,
    package: &ValidatedPackage,
    warnings: &mut Vec<AnalysisWarning>,
) -> SourceInformation {
    let raw_application = main.metadata.get("Application").cloned();
    let project_version = project_settings
        .root
        .as_ref()
        .and_then(|root| string_value(root.get("version")));
    let (application, application_name, application_version) =
        parse_application(raw_application.as_deref(), project_version);
    let has_metadata = main.has_bambu_namespace
        || project_settings.root.is_some()
        || package.contains(MODEL_SETTINGS_PATH);
    let dialect = match application {
        SourceApplication::BambuStudio => ProjectDialect::BambuStudioProject,
        SourceApplication::OrcaSlicer => ProjectDialect::OrcaSlicerProject,
        SourceApplication::SnapmakerOrca => ProjectDialect::SnapmakerOrcaProject,
        SourceApplication::Unknown if has_metadata => ProjectDialect::Unknown,
        SourceApplication::Unknown => ProjectDialect::Standard3mf,
    };
    let support = match (application, application_version.as_deref()) {
        (SourceApplication::BambuStudio, Some("02.06.00.51")) => DialectSupport::Supported,
        (SourceApplication::SnapmakerOrca, Some("2.3.5" | "02.03.05")) => DialectSupport::Supported,
        (SourceApplication::BambuStudio | SourceApplication::OrcaSlicer, _) => {
            DialectSupport::Experimental
        }
        (SourceApplication::SnapmakerOrca, _) => DialectSupport::Experimental,
        (SourceApplication::Unknown, _) if has_metadata => DialectSupport::Unsupported,
        (SourceApplication::Unknown, _) => DialectSupport::Limited,
    };
    if matches!(
        support,
        DialectSupport::Experimental | DialectSupport::Unsupported
    ) {
        warnings.push(AnalysisWarning::new(
            WarningCode::ExperimentalDialect,
            format!(
                "Source application/version is not a verified dialect fixture: {} {}",
                application_name.as_deref().unwrap_or("unknown"),
                application_version.as_deref().unwrap_or("unknown")
            ),
        ));
    }
    let sliced_artifact_entries =
        collect_sliced_artifact_entries(package.entries.keys().map(String::as_str));
    let dialect_version = main
        .metadata
        .get("BambuStudio:3mfVersion")
        .or_else(|| main.metadata.get("OrcaSlicer:3mfVersion"))
        .cloned();
    SourceInformation {
        application,
        application_name,
        application_version,
        title: main.metadata.get("Title").cloned().and_then(nonempty),
        dialect,
        dialect_version,
        support,
        has_bambu_or_orca_metadata: has_metadata,
        has_sliced_artifacts: !sliced_artifact_entries.is_empty(),
        sliced_artifact_entries,
    }
}

fn collect_sliced_artifact_entries<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| {
            matches!(
                stale_artifact_kind(path),
                Some(
                    StaleArtifactKind::Toolpath
                        | StaleArtifactKind::ToolpathChecksum
                        | StaleArtifactKind::SliceMetadata
                        | StaleArtifactKind::SlicePreview
                )
            )
        })
        .map(str::to_owned)
        .collect()
}

fn parse_application(
    raw: Option<&str>,
    project_version: Option<String>,
) -> (SourceApplication, Option<String>, Option<String>) {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return (SourceApplication::Unknown, None, project_version);
    };
    let lower = raw.to_ascii_lowercase();
    let application = if lower.contains("snapmaker") && lower.contains("orca") {
        SourceApplication::SnapmakerOrca
    } else if lower.contains("bambustudio") || lower.contains("bambu studio") {
        SourceApplication::BambuStudio
    } else if lower.contains("orcaslicer") || lower.contains("orca slicer") {
        SourceApplication::OrcaSlicer
    } else {
        SourceApplication::Unknown
    };
    let (name, inline_version) = raw
        .rsplit_once('-')
        .filter(|(_, version)| version.chars().any(|character| character.is_ascii_digit()))
        .map_or((raw.to_owned(), None), |(name, version)| {
            (name.to_owned(), Some(version.to_owned()))
        });
    (application, Some(name), inline_version.or(project_version))
}

fn add_alternative_plate_warning(plates: &[PlateAnalysis], warnings: &mut Vec<AnalysisWarning>) {
    let joint_plates: Vec<_> = plates
        .iter()
        .filter(|plate| {
            plate
                .name
                .as_deref()
                .is_some_and(|name| name.to_ascii_lowercase().contains("joint"))
        })
        .map(|plate| plate.id)
        .collect();
    if joint_plates.len() >= 2 {
        warnings.push(AnalysisWarning::new(
            WarningCode::PossibleAlternativePlates,
            format!(
                "Plates {joint_plates:?} appear to contain alternative joint sets; user selection is required"
            ),
        ));
    }
}

fn parse_volume_type(raw: &str) -> VolumeType {
    match raw.to_ascii_lowercase().as_str() {
        "normal_part" | "model_part" => VolumeType::NormalPart,
        "negative_part" | "negative_volume" => VolumeType::NegativePart,
        "modifier" | "parameter_modifier" | "modifier_part" => VolumeType::Modifier,
        "support_blocker" => VolumeType::SupportBlocker,
        "support_enforcer" => VolumeType::SupportEnforcer,
        _ => VolumeType::Unknown(raw.to_owned()),
    }
}

fn positional_string_array(value: Option<&Value>) -> Vec<Option<String>> {
    match value {
        Some(Value::Array(values)) => values.iter().map(value_as_string).collect(),
        Some(Value::String(value)) if value.contains(';') => value
            .split(';')
            .map(str::trim)
            .map(str::to_owned)
            .map(Some)
            .collect(),
        Some(value) => vec![value_as_string(value)],
        None => Vec::new(),
    }
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value.and_then(value_as_string).and_then(nonempty)
}

fn value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "1" } else { "0" }.into()),
        _ => None,
    }
}

fn bool_value(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_i64().map(|value| value != 0),
        Value::String(value) => parse_bool(value).ok(),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => Err(format!("invalid boolean value {value:?}")),
    }
}

fn parse_positive_f64(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn parse_slot(value: &str) -> Option<u16> {
    value.trim().parse().ok().filter(|slot| *slot > 0)
}

fn valid_color(value: &str) -> bool {
    matches!(value.len(), 7 | 9)
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn nonempty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn unit_scale_mm(value: &str, entry: &str) -> Result<f64, AnalysisError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "micron" => Ok(0.001),
        "millimeter" => Ok(1.0),
        "centimeter" => Ok(10.0),
        "inch" => Ok(25.4),
        "foot" => Ok(304.8),
        "meter" => Ok(1_000.0),
        _ => Err(AnalysisError::InvalidStructure(format!(
            "unsupported 3MF unit {value:?} in {entry}"
        ))),
    }
}

fn parse_vertex<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    entry: &str,
    unit_scale_mm: f64,
) -> Result<[f64; 3], AnalysisError> {
    let mut point: [Option<f64>; 3] = [None; 3];
    let mut attributes = event.attributes();
    attributes.with_checks(false);
    let mut seen = SeenAttributeNames::new();
    for attribute in attributes {
        let attribute = attribute.map_err(|error| xml_error(entry, error))?;
        if !seen.insert(attribute.key) {
            return Err(xml_error(
                entry,
                format!(
                    "duplicated attribute {}",
                    String::from_utf8_lossy(attribute.key.as_ref())
                ),
            ));
        }
        let axis = match local_name(attribute.key.as_ref()) {
            b"x" => 0,
            b"y" => 1,
            b"z" => 2,
            _ => continue,
        };
        if point[axis].is_some() {
            // `xml_attr` historically selected the first local-name match;
            // preserve that behavior for unusual namespace aliases.
            continue;
        }
        let raw = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| xml_error(entry, error))?;
        let value = raw.parse::<f64>().map_err(|_| {
            AnalysisError::InvalidStructure(format!("invalid vertex coordinate {raw:?} in {entry}"))
        })?;
        let millimeters = value * unit_scale_mm;
        if !millimeters.is_finite() {
            return Err(AnalysisError::InvalidStructure(format!(
                "vertex coordinate {raw:?} is not finite in {entry}"
            )));
        }
        point[axis] = Some(millimeters);
    }
    let required = |axis: usize, name: &str| {
        point[axis].ok_or_else(|| xml_error(entry, format!("missing attribute {name}")))
    };
    Ok([required(0, "x")?, required(1, "y")?, required(2, "z")?])
}

/// Allocation-free duplicate-name validation for ordinary mesh tags. The
/// upstream iterator stores every key range in a heap `Vec` when checks are
/// enabled; Withered Foxy has more than twenty million vertex/triangle tags.
/// Eight inline names cover normal 3MF mesh elements, while unusual tags keep
/// the same validation semantics through the bounded overflow vector.
struct SeenAttributeNames<'a> {
    inline: [Option<QName<'a>>; 8],
    inline_len: usize,
    overflow: Vec<QName<'a>>,
}

impl<'a> SeenAttributeNames<'a> {
    fn new() -> Self {
        Self {
            inline: [None; 8],
            inline_len: 0,
            overflow: Vec::new(),
        }
    }

    fn insert(&mut self, name: QName<'a>) -> bool {
        if self.inline[..self.inline_len]
            .iter()
            .flatten()
            .chain(&self.overflow)
            .any(|existing| *existing == name)
        {
            return false;
        }
        if self.inline_len < self.inline.len() {
            self.inline[self.inline_len] = Some(name);
            self.inline_len += 1;
        } else {
            self.overflow.push(name);
        }
        true
    }
}

fn extend_bounds(
    bounds: &mut Option<AxisAlignedBounds>,
    point: [f64; 3],
) -> Result<(), AnalysisError> {
    if !point.into_iter().all(f64::is_finite) {
        return Err(AnalysisError::InvalidStructure(
            "geometry contains a non-finite vertex".into(),
        ));
    }
    *bounds = Some(match *bounds {
        Some(existing) => AxisAlignedBounds {
            min: [
                existing.min[0].min(point[0]),
                existing.min[1].min(point[1]),
                existing.min[2].min(point[2]),
            ],
            max: [
                existing.max[0].max(point[0]),
                existing.max[1].max(point[1]),
                existing.max[2].max(point[2]),
            ],
        },
        None => AxisAlignedBounds {
            min: point,
            max: point,
        },
    });
    Ok(())
}

fn parse_transform(
    value: &str,
    entry: &str,
    translation_scale_mm: f64,
) -> Result<Transform3mf, AnalysisError> {
    let mut values: Vec<f64> = value
        .split_ascii_whitespace()
        .map(|component| {
            component.parse::<f64>().map_err(|_| {
                AnalysisError::InvalidStructure(format!(
                    "invalid transform component {component:?} in {entry}"
                ))
            })
        })
        .collect::<Result<_, _>>()?;
    if values.len() != 12 || values.iter().any(|value| !value.is_finite()) {
        return Err(AnalysisError::InvalidStructure(format!(
            "3MF transform in {entry} must contain 12 finite numbers"
        )));
    }
    for translation in &mut values[9..] {
        *translation *= translation_scale_mm;
        if !translation.is_finite() {
            return Err(AnalysisError::InvalidStructure(format!(
                "3MF transform translation in {entry} is not finite after unit conversion"
            )));
        }
    }
    let values: [f64; 12] = values.try_into().expect("length checked");
    Ok(Transform3mf { values })
}

fn resolve_package_target(source_part: &str, target: &str) -> Result<String, AnalysisError> {
    if target.is_empty() || target.contains('\\') || target.contains('\0') {
        return Err(AnalysisError::InvalidStructure(format!(
            "unsafe package target {target:?}"
        )));
    }
    if target.contains("://") {
        return Err(AnalysisError::InvalidStructure(format!(
            "external URI used as an internal package target: {target:?}"
        )));
    }
    let mut segments = Vec::new();
    if !target.starts_with('/') && !source_part.is_empty() {
        let parent = source_part
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        segments.extend(parent.split('/').filter(|segment| !segment.is_empty()));
    }
    for segment in target.trim_start_matches('/').split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(AnalysisError::InvalidStructure(format!(
                        "package target escapes the archive root: {target:?}"
                    )));
                }
            }
            segment if segment.contains(':') => {
                return Err(AnalysisError::InvalidStructure(format!(
                    "unsafe package target segment {segment:?}"
                )));
            }
            segment => segments.push(segment),
        }
    }
    if segments.is_empty() {
        return Err(AnalysisError::InvalidStructure(format!(
            "empty resolved package target {target:?}"
        )));
    }
    Ok(segments.join("/"))
}

fn parse_u32(value: &str, entry: &str) -> Result<u32, AnalysisError> {
    value.parse().map_err(|_| {
        AnalysisError::InvalidStructure(format!("invalid unsigned ID {value:?} in {entry}"))
    })
}

fn required_u32_attr<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    name: &[u8],
    entry: &str,
) -> Result<u32, AnalysisError> {
    let value = required_attr(reader, event, name, entry)?;
    parse_u32(&value, entry)
}

fn required_attr<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    name: &[u8],
    entry: &str,
) -> Result<String, AnalysisError> {
    xml_attr(reader, event, name, entry)?.ok_or_else(|| {
        xml_error(
            entry,
            format!("missing attribute {}", String::from_utf8_lossy(name)),
        )
    })
}

fn xml_attr<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    wanted: &[u8],
    entry: &str,
) -> Result<Option<String>, AnalysisError> {
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| xml_error(entry, error))?;
        if local_eq(attribute.key.as_ref(), wanted) {
            return attribute
                .decode_and_unescape_value(reader.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|error| xml_error(entry, error));
        }
    }
    Ok(None)
}

fn read_xml_event<'a, R: BufRead>(
    reader: &mut Reader<R>,
    buffer: &'a mut Vec<u8>,
    entry: &str,
) -> Result<Event<'a>, AnalysisError> {
    let event = reader
        .read_event_into(buffer)
        .map_err(|error| xml_error(entry, error))?;
    if let Event::Text(text) = &event
        && text.as_ref().contains(&b'&')
    {
        let decoded = text.decode().map_err(|error| xml_error(entry, error))?;
        quick_xml::escape::unescape(&decoded).map_err(|error| xml_error(entry, error))?;
    }
    Ok(event)
}

fn checked_depth(
    current: usize,
    limits: &AnalysisLimits,
    entry: &str,
) -> Result<usize, AnalysisError> {
    let next = current.saturating_add(1);
    if next > limits.max_xml_depth {
        return Err(xml_error(
            entry,
            format!("XML nesting exceeds {}", limits.max_xml_depth),
        ));
    }
    Ok(next)
}

fn xml_error(entry: &str, error: impl std::fmt::Display) -> AnalysisError {
    AnalysisError::InvalidXml {
        entry: entry.to_owned(),
        message: error.to_string(),
    }
}

fn local_eq(name: &[u8], expected: &[u8]) -> bool {
    local_name(name) == expected
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::{NamedTempFile, tempdir};
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    #[test]
    fn path_validation_rejects_cross_platform_traversal() {
        let limits = AnalysisLimits::default();
        for unsafe_name in [
            "../escape",
            "a/../../escape",
            "/absolute",
            "C:/drive",
            "a\\..\\escape",
            "a//b",
            "./a",
        ] {
            assert!(validate_entry_name(unsafe_name, false, &limits).is_err());
        }
        assert!(validate_entry_name("3D/Objects/part.model", false, &limits).is_ok());
        assert!(validate_entry_name("Metadata/", true, &limits).is_ok());
    }

    #[test]
    fn snapshot_identity_is_computed_from_the_copied_bytes() {
        let source = NamedTempFile::new().unwrap();
        fs::write(source.path(), b"PK\x03\x04payload").unwrap();
        let mut source_file = File::open(source.path()).unwrap();
        let mut snapshot = tempfile::tempfile().unwrap();

        let identity = copy_and_hash_snapshot(&mut source_file, &mut snapshot, 1024).unwrap();

        assert_eq!(identity.byte_size, 11);
        assert_eq!(
            identity.sha256,
            hex::encode(Sha256::digest(b"PK\x03\x04payload"))
        );
        snapshot.seek(SeekFrom::Start(0)).unwrap();
        let mut copied = Vec::new();
        snapshot.read_to_end(&mut copied).unwrap();
        assert_eq!(copied, b"PK\x03\x04payload");
    }

    #[test]
    fn snapshot_copy_stops_at_the_compressed_archive_limit() {
        let source = NamedTempFile::new().unwrap();
        fs::write(source.path(), b"PK\x03\x04payload").unwrap();
        let mut source_file = File::open(source.path()).unwrap();
        let mut snapshot = tempfile::tempfile().unwrap();

        assert!(matches!(
            copy_and_hash_snapshot(&mut source_file, &mut snapshot, 10),
            Err(AnalysisError::InputArchiveTooLarge {
                actual: 11,
                limit: 10
            })
        ));
    }

    #[test]
    fn parallel_geometry_parsing_matches_single_worker_output() {
        let snapshot = NamedTempFile::new().unwrap();
        let mut writer = zip::ZipWriter::new(snapshot.reopen().unwrap());
        for index in 0..12_u32 {
            let path = format!("3D/Objects/part-{index:02}.model");
            let paint = if index % 2 == 0 {
                " paint_color=\"8\""
            } else {
                ""
            };
            let model = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources><object id="{index}"><mesh>
    <vertices><vertex x="{index}" y="0" z="0"/><vertex x="1" y="2" z="3"/><vertex x="4" y="5" z="6"/></vertices>
    <triangles><triangle v1="0" v2="1" v3="2"{paint}/></triangles>
  </mesh></object></resources>
</model>"#,
            );
            writer
                .start_file(
                    path,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(model.as_bytes()).unwrap();
        }
        writer.finish().unwrap();

        let mut archive = ZipArchive::new(snapshot.reopen().unwrap()).unwrap();
        let entries = (0..archive.len())
            .map(|index| {
                let entry = archive.by_index(index).unwrap();
                (
                    entry.name().to_owned(),
                    EntryInfo {
                        index,
                        uncompressed_size: entry.size(),
                    },
                )
            })
            .collect::<Vec<_>>();
        let snapshot_file = Arc::new(snapshot.reopen().unwrap());
        let limits = AnalysisLimits::default();

        let serial =
            parse_geometry_entries_with_worker_count(&snapshot_file, entries.clone(), &limits, 1)
                .unwrap();
        let parallel =
            parse_geometry_entries_with_worker_count(&snapshot_file, entries, &limits, 4).unwrap();

        assert_eq!(parallel, serial);
    }

    #[cfg(unix)]
    #[test]
    fn source_guard_detects_atomic_path_replacement() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("source.3mf");
        let replacement = directory.path().join("replacement.3mf");
        fs::write(&path, b"PK\x03\x04original").unwrap();
        fs::write(&replacement, b"PK\x03\x04replaced").unwrap();
        let source = File::open(&path).unwrap();
        let fingerprint = SourceFingerprint::from_metadata(&source.metadata().unwrap());
        let guard = SourceGuard {
            path: path.clone(),
            source,
            fingerprint,
        };
        fs::rename(&replacement, &path).unwrap();

        assert!(matches!(
            guard.ensure_unchanged(),
            Err(AnalysisError::SourceChangedDuringAnalysis(changed)) if changed == path
        ));
    }

    #[test]
    fn model_xml_entry_detection_is_narrow() {
        assert!(is_model_xml_entry("3D/3dmodel.model"));
        assert!(is_model_xml_entry("3D/Objects/part.MODEL"));
        assert!(!is_model_xml_entry("Metadata/model.model"));
        assert!(!is_model_xml_entry("3D/Objects/part.model.json"));
        assert!(!is_model_xml_entry("3D/Objects/part.xml"));
    }

    #[test]
    fn package_target_resolution_is_root_bounded() {
        assert_eq!(
            resolve_package_target(MAIN_MODEL_PATH, "Objects/part.model").unwrap(),
            "3D/Objects/part.model"
        );
        assert_eq!(
            resolve_package_target(MAIN_MODEL_PATH, "/3D/Objects/part.model").unwrap(),
            "3D/Objects/part.model"
        );
        assert!(resolve_package_target("", "../escape").is_err());
    }

    #[test]
    fn relationship_source_paths_follow_opc_part_naming() {
        assert_eq!(
            source_path_for_relationship(ROOT_RELATIONSHIPS_PATH).unwrap(),
            ""
        );
        assert_eq!(
            source_path_for_relationship(MAIN_RELATIONSHIPS_PATH).unwrap(),
            MAIN_MODEL_PATH
        );
        assert_eq!(
            source_path_for_relationship("3D/Objects/_rels/part.model.rels").unwrap(),
            "3D/Objects/part.model"
        );
        assert!(source_path_for_relationship("Metadata/not-in-rels.rels").is_err());
    }

    #[test]
    fn material_color_identity_preserves_material() {
        let filaments = vec![
            DeclaredFilament {
                slot: 1,
                material: Some("PLA".into()),
                color: Some("#000000".into()),
                preset: None,
                used: false,
            },
            DeclaredFilament {
                slot: 2,
                material: Some("PETG".into()),
                color: Some("#000000".into()),
                preset: None,
                used: false,
            },
        ];
        let roles = BTreeMap::from([
            (1, BTreeSet::from([MaterialRole::Model])),
            (2, BTreeSet::from([MaterialRole::Model])),
        ]);
        assert_eq!(effective_material_colors(&roles, &filaments).len(), 2);
    }

    #[test]
    fn material_color_identity_preserves_distinct_roles() {
        let filaments = vec![
            DeclaredFilament {
                slot: 1,
                material: Some("PLA".into()),
                color: Some("#808080".into()),
                preset: None,
                used: false,
            },
            DeclaredFilament {
                slot: 2,
                material: Some("PLA".into()),
                color: Some("#808080".into()),
                preset: None,
                used: false,
            },
        ];
        let roles = BTreeMap::from([
            (1, BTreeSet::from([MaterialRole::Model])),
            (2, BTreeSet::from([MaterialRole::Support])),
        ]);

        let effective = effective_material_colors(&roles, &filaments);

        assert_eq!(effective.len(), 2);
        assert!(
            effective
                .iter()
                .any(|color| color.roles == vec![MaterialRole::Model])
        );
        assert!(
            effective
                .iter()
                .any(|color| color.roles == vec![MaterialRole::Support])
        );
    }

    #[test]
    fn material_color_identity_preserves_distinct_source_profiles() {
        let filaments = vec![
            DeclaredFilament {
                slot: 1,
                material: Some("PLA".into()),
                color: Some("#808080".into()),
                preset: Some("PLA Basic @U1".into()),
                used: false,
            },
            DeclaredFilament {
                slot: 2,
                material: Some("PLA".into()),
                color: Some("#808080".into()),
                preset: Some("PLA Matte @U1".into()),
                used: false,
            },
        ];
        let roles = BTreeMap::from([
            (1, BTreeSet::from([MaterialRole::Model])),
            (2, BTreeSet::from([MaterialRole::Model])),
        ]);

        let effective = effective_material_colors(&roles, &filaments);

        assert_eq!(effective.len(), 2);
        assert_eq!(effective[0].source_slots, vec![1]);
        assert_eq!(effective[0].source_profile_ids, vec!["PLA Basic @U1"]);
        assert_eq!(effective[1].source_slots, vec![2]);
        assert_eq!(effective[1].source_profile_ids, vec!["PLA Matte @U1"]);
    }

    #[test]
    fn legacy_effective_material_color_defaults_source_profiles() {
        let legacy = serde_json::json!({
            "source_slots": [1],
            "material": "PLA",
            "color": "#808080",
            "roles": ["model"]
        });

        let effective: EffectiveMaterialColor = serde_json::from_value(legacy).unwrap();

        assert!(effective.source_profile_ids.is_empty());
    }

    #[test]
    fn standard_3mf_fallback_numbers_instances_per_object() {
        let object = |_| MainObject {
            name: None,
            has_mesh: false,
            mesh_bounds: None,
            components: Vec::new(),
        };
        let item = |source_index, object_id| BuildItem {
            source_index,
            object_id,
            path: None,
            printable: true,
            transform: None,
        };
        let main = MainModel {
            objects: BTreeMap::from([(10, object(10)), (20, object(20))]),
            build_items: vec![item(0, 10), item(1, 20), item(2, 10)],
            ..MainModel::default()
        };

        let settings = fallback_model_settings(&main);
        let instances = &settings.plates[0].instances;
        assert_eq!(instances[0].instance_id, Some(0));
        assert_eq!(instances[1].instance_id, Some(0));
        assert_eq!(instances[2].instance_id, Some(1));

        let mut warnings = Vec::new();
        let (_, plates) = build_objects_and_plates(
            &main,
            &settings,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            &mut warnings,
        );
        assert_eq!(
            plates[0]
                .instances
                .iter()
                .map(|instance| instance.source_build_item_index)
                .collect::<Vec<_>>(),
            [Some(0), Some(1), Some(2)]
        );
        assert!(
            warnings
                .iter()
                .all(|warning| warning.code != WarningCode::UnsafeBuildInstanceGraph)
        );
    }

    #[test]
    fn duplicate_build_instance_mapping_is_marked_unsafe_for_conversion() {
        let object = MainObject {
            name: None,
            has_mesh: false,
            mesh_bounds: None,
            components: Vec::new(),
        };
        let main = MainModel {
            objects: BTreeMap::from([(10, object)]),
            build_items: vec![
                BuildItem {
                    source_index: 0,
                    object_id: 10,
                    path: None,
                    printable: true,
                    transform: None,
                },
                BuildItem {
                    source_index: 1,
                    object_id: 10,
                    path: None,
                    printable: true,
                    transform: None,
                },
            ],
            ..MainModel::default()
        };
        let mut settings = fallback_model_settings(&main);
        settings.plates[0].instances[1].instance_id = Some(0);
        let mut warnings = Vec::new();
        let _ = build_objects_and_plates(
            &main,
            &settings,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            &mut warnings,
        );

        assert!(
            warnings
                .iter()
                .any(|warning| warning.code == WarningCode::UnsafeBuildInstanceGraph)
        );
    }

    #[test]
    fn setting_arrays_preserve_slot_positions() {
        let values = serde_json::json!(["first", null, "third"]);
        assert_eq!(
            positional_string_array(Some(&values)),
            vec![Some("first".into()), None, Some("third".into())]
        );
    }

    #[test]
    fn sliced_artifact_summary_uses_the_shared_stale_classifier() {
        let entries = collect_sliced_artifact_entries([
            "Metadata/slice_info.config",
            "Metadata/plate_7.json",
            "Metadata/top_7.png",
            "Metadata/plate_7.gcode.md5",
            "Metadata/plate_7.bgcode",
            "Auxiliaries/.thumbnails/thumbnail_3mf.png",
            "Metadata/project_settings.config",
        ]);

        assert_eq!(
            entries,
            vec![
                "Metadata/slice_info.config",
                "Metadata/plate_7.json",
                "Metadata/top_7.png",
                "Metadata/plate_7.gcode.md5",
                "Metadata/plate_7.bgcode",
            ]
        );
    }

    #[test]
    fn component_cycles_produce_unknown_bounds() {
        let graph = BTreeMap::from([(
            (MAIN_MODEL_PATH.to_owned(), 1),
            ResourceObject {
                has_mesh: false,
                mesh_bounds: None,
                components: vec![ComponentReference {
                    object_id: 1,
                    path: None,
                    transform: None,
                }],
            },
        )]);
        assert_eq!(
            resolve_resource_bounds(&graph, MAIN_MODEL_PATH, 1, Transform3mf::IDENTITY),
            None
        );
    }

    #[test]
    fn transforms_follow_3mf_field_order_and_units() {
        let bounds = AxisAlignedBounds {
            min: [0.0, 0.0, 0.0],
            max: [2.0, 3.0, 4.0],
        };
        let transform = Transform3mf {
            values: [
                0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0,
            ],
        };
        let transformed = bounds.transformed(transform).unwrap();
        assert_eq!(transformed.min, [7.0, 20.0, 30.0]);
        assert_eq!(transformed.max, [10.0, 22.0, 34.0]);
        assert_eq!(unit_scale_mm("inch", "test").unwrap(), 25.4);
        assert!(unit_scale_mm("parsec", "test").is_err());
    }
}
