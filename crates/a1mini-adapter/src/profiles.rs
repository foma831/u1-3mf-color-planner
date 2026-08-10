use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::{
    A1MINI_PROFILE_BASELINE, A1MINI_PROFILE_MANIFEST_SHA256, A1MINI_PROFILE_PACK_VERSION,
    A1MiniError, A1MiniMaterial, AdapterContext, GENERIC_PETG_PROFILE_PATH,
    GENERIC_PLA_PROFILE_PATH, MACHINE_PROFILE_PATH, PROCESS_PROFILE_PATH,
};

#[derive(Clone, Debug)]
pub(crate) struct VerifiedProfileStore {
    documents: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedTargetProfiles {
    pub machine: BTreeMap<String, Value>,
    pub process: BTreeMap<String, Value>,
    pub filament: ResolvedFilamentProfile,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedFilamentProfile {
    pub name: String,
    pub setting_id: String,
    pub filament_id: String,
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct VendorManifest {
    name: String,
    version: String,
}

pub(crate) fn load_target_profiles(
    context: &AdapterContext,
    material: A1MiniMaterial,
) -> Result<ResolvedTargetProfiles, A1MiniError> {
    let store = load_verified_profile_store(context)?;
    let machine = resolve_profile_chain(&store, MACHINE_PROFILE_PATH)?;
    let process = resolve_profile_chain(&store, PROCESS_PROFILE_PATH)?;
    let filament_path = match material {
        A1MiniMaterial::Pla => GENERIC_PLA_PROFILE_PATH,
        A1MiniMaterial::Petg => GENERIC_PETG_PROFILE_PATH,
    };
    let values = resolve_profile_chain(&store, filament_path)?;
    let expected_name = match material {
        A1MiniMaterial::Pla => "Generic PLA @BBL A1M",
        A1MiniMaterial::Petg => "Generic PETG @BBL A1M",
    };
    let name = values
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| A1MiniError::Capability(format!("profile {filament_path} has no name")))?
        .to_owned();
    if name != expected_name {
        return Err(A1MiniError::Capability(format!(
            "profile {filament_path} resolved to {name:?}, expected {expected_name:?}"
        )));
    }
    let setting_id = values
        .get("setting_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            A1MiniError::Capability(format!("profile {filament_path} has no leaf setting_id"))
        })?
        .to_owned();
    let filament_id = values
        .get("filament_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            A1MiniError::Capability(format!(
                "profile {filament_path} has no material filament_id"
            ))
        })?
        .to_owned();
    Ok(ResolvedTargetProfiles {
        machine,
        process,
        filament: ResolvedFilamentProfile {
            name,
            setting_id,
            filament_id,
            values,
        },
    })
}

fn load_verified_profile_store(
    context: &AdapterContext,
) -> Result<VerifiedProfileStore, A1MiniError> {
    let manifest_before = fs::read(&context.manifest_path).map_err(|source| A1MiniError::Read {
        path: context.manifest_path.clone(),
        source,
    })?;
    verify_manifest(&manifest_before)?;
    let mut documents = BTreeMap::new();
    for (relative_path, expected_hash) in A1MINI_PROFILE_BASELINE {
        let path = context.profiles_root.join(relative_path);
        let bytes = fs::read(&path).map_err(|source| A1MiniError::Read {
            path: path.clone(),
            source,
        })?;
        let actual_hash = format!("{:x}", Sha256::digest(&bytes));
        if actual_hash != *expected_hash {
            return Err(A1MiniError::Capability(format!(
                "required A1 mini profile {relative_path} changed after capability inspection (found {actual_hash}, expected {expected_hash})"
            )));
        }
        let document =
            serde_json::from_slice::<BTreeMap<String, Value>>(&bytes).map_err(|source| {
                A1MiniError::Json {
                    path: path.to_string_lossy().into_owned(),
                    source,
                }
            })?;
        documents.insert((*relative_path).to_owned(), document);
    }
    let manifest_after = fs::read(&context.manifest_path).map_err(|source| A1MiniError::Read {
        path: context.manifest_path.clone(),
        source,
    })?;
    verify_manifest(&manifest_after)?;
    if manifest_before != manifest_after {
        return Err(A1MiniError::Capability(
            "the active BBL manifest changed while target profiles were being loaded".into(),
        ));
    }
    Ok(VerifiedProfileStore { documents })
}

fn verify_manifest(bytes: &[u8]) -> Result<(), A1MiniError> {
    let actual_hash = format!("{:x}", Sha256::digest(bytes));
    if actual_hash != A1MINI_PROFILE_MANIFEST_SHA256 {
        return Err(A1MiniError::Capability(format!(
            "the active BBL manifest changed after capability inspection (found {actual_hash}, expected {A1MINI_PROFILE_MANIFEST_SHA256})"
        )));
    }
    let manifest =
        serde_json::from_slice::<VendorManifest>(bytes).map_err(|source| A1MiniError::Json {
            path: "active BBL.json".into(),
            source,
        })?;
    if manifest.name != "Bambulab" || manifest.version != A1MINI_PROFILE_PACK_VERSION {
        return Err(A1MiniError::Capability(format!(
            "the active BBL manifest changed identity (vendor {:?}, version {:?})",
            manifest.name, manifest.version
        )));
    }
    Ok(())
}

fn resolve_profile_chain(
    store: &VerifiedProfileStore,
    relative_path: &str,
) -> Result<BTreeMap<String, Value>, A1MiniError> {
    let category = Path::new(relative_path).parent().ok_or_else(|| {
        A1MiniError::Capability(format!("profile path {relative_path} has no category"))
    })?;
    let mut chain = Vec::new();
    let mut current = relative_path.to_owned();
    let mut seen = BTreeSet::new();
    loop {
        let document = store.documents.get(&current).ok_or_else(|| {
            A1MiniError::Capability(format!(
                "profile {current} is absent from the verified A1 mini baseline"
            ))
        })?;
        let name = document
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&current)
            .to_owned();
        if !seen.insert(name.clone()) {
            return Err(A1MiniError::Capability(format!(
                "profile inheritance cycle at {name}"
            )));
        }
        let parent = document
            .get("inherits")
            .and_then(Value::as_str)
            .map(str::to_owned);
        chain.push(document.clone());
        let Some(parent) = parent else { break };
        let matches = store
            .documents
            .iter()
            .filter(|(candidate_path, candidate)| {
                Path::new(candidate_path.as_str()).parent() == Some(category)
                    && candidate.get("name").and_then(Value::as_str) == Some(parent.as_str())
            })
            .map(|(candidate_path, _)| candidate_path.clone())
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(A1MiniError::Capability(format!(
                "verified profile {name} inherits {parent:?}, which resolves to {} baseline files",
                matches.len()
            )));
        }
        current = matches[0].clone();
    }
    let mut resolved = BTreeMap::new();
    for document in chain.into_iter().rev() {
        resolved.extend(document);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_baseline_contains_both_material_families() {
        let paths = A1MINI_PROFILE_BASELINE
            .iter()
            .map(|(path, _)| *path)
            .collect::<BTreeSet<_>>();
        assert!(paths.contains(GENERIC_PLA_PROFILE_PATH));
        assert!(paths.contains(GENERIC_PETG_PROFILE_PATH));
        assert_eq!(paths.len(), A1MINI_PROFILE_BASELINE.len());
    }
}
