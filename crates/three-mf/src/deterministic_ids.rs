use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;
use uuid::Uuid;

const ALLOCATOR_NAMESPACE_NAME: &[u8] =
    b"https://u1-3mf-color-planner.local/production-identifiers/v1";

/// Deterministic Production Extension identifiers scoped to immutable source
/// bytes and one canonical output-job ID.
#[derive(Clone, Debug)]
pub struct DeterministicProductionIds {
    job_namespace: Uuid,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DeterministicIdError {
    #[error("source SHA-256 must contain 64 lowercase hexadecimal characters")]
    InvalidSourceSha256,
    #[error("canonical job ID must not be empty or contain NUL")]
    InvalidJobId,
    #[error("identifier kind and stable key must not be empty or contain NUL")]
    InvalidStableKey,
    #[error("stable resource key {0:?} is duplicated")]
    DuplicateStableKey(String),
    #[error("resource key count exceeds the 3MF resource-ID range")]
    ResourceIdRangeExhausted,
}

impl DeterministicProductionIds {
    pub fn new(source_sha256: &str, canonical_job_id: &str) -> Result<Self, DeterministicIdError> {
        if source_sha256.len() != 64
            || !source_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DeterministicIdError::InvalidSourceSha256);
        }
        if canonical_job_id.trim().is_empty() || canonical_job_id.contains('\0') {
            return Err(DeterministicIdError::InvalidJobId);
        }
        let allocator_namespace = Uuid::new_v5(&Uuid::NAMESPACE_URL, ALLOCATOR_NAMESPACE_NAME);
        let scope = format!("{source_sha256}\0{canonical_job_id}");
        Ok(Self {
            job_namespace: Uuid::new_v5(&allocator_namespace, scope.as_bytes()),
        })
    }

    /// Returns a canonical lowercase UUID for an element with a stable semantic
    /// key. Call order does not affect the result.
    pub fn uuid(&self, kind: &str, stable_key: &str) -> Result<String, DeterministicIdError> {
        if kind.trim().is_empty()
            || stable_key.trim().is_empty()
            || kind.contains('\0')
            || stable_key.contains('\0')
        {
            return Err(DeterministicIdError::InvalidStableKey);
        }
        Ok(Uuid::new_v5(
            &self.job_namespace,
            format!("{kind}\0{stable_key}").as_bytes(),
        )
        .hyphenated()
        .to_string())
    }

    /// Assigns 3MF resource IDs in lexical stable-key order. The same set of
    /// keys therefore receives the same IDs regardless of input iteration order.
    pub fn resource_ids(
        &self,
        stable_keys: impl IntoIterator<Item = String>,
    ) -> Result<BTreeMap<String, u32>, DeterministicIdError> {
        let mut ordered = BTreeSet::new();
        for key in stable_keys {
            if key.trim().is_empty() || key.contains('\0') {
                return Err(DeterministicIdError::InvalidStableKey);
            }
            if !ordered.insert(key.clone()) {
                return Err(DeterministicIdError::DuplicateStableKey(key));
            }
        }
        if ordered.len() as u64 >= 2_147_483_648_u64 {
            return Err(DeterministicIdError::ResourceIdRangeExhausted);
        }
        ordered
            .into_iter()
            .enumerate()
            .map(|(index, key)| {
                let id = u32::try_from(index + 1)
                    .map_err(|_| DeterministicIdError::ResourceIdRangeExhausted)?;
                Ok((key, id))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_and_resource_ids_are_stable_and_order_independent() {
        let allocator = DeterministicProductionIds::new(&"a".repeat(64), "u1/direct/batch-01")
            .expect("valid deterministic scope");
        let first_uuid = allocator.uuid("component", "source-unit-42").unwrap();
        assert_eq!(
            first_uuid,
            allocator.uuid("component", "source-unit-42").unwrap()
        );
        assert_ne!(
            first_uuid,
            allocator.uuid("object", "source-unit-42").unwrap()
        );
        assert_eq!(
            Uuid::parse_str(&first_uuid).unwrap().to_string(),
            first_uuid
        );

        let first = allocator
            .resource_ids(["z".to_owned(), "a".to_owned()])
            .unwrap();
        let second = allocator
            .resource_ids(["a".to_owned(), "z".to_owned()])
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first["a"], 1);
        assert_eq!(first["z"], 2);
    }

    #[test]
    fn invalid_scope_and_duplicate_keys_fail_closed() {
        assert!(matches!(
            DeterministicProductionIds::new("ABC", "job"),
            Err(DeterministicIdError::InvalidSourceSha256)
        ));
        let allocator = DeterministicProductionIds::new(&"b".repeat(64), "job").unwrap();
        assert!(matches!(
            allocator.resource_ids(["same".to_owned(), "same".to_owned()]),
            Err(DeterministicIdError::DuplicateStableKey(_))
        ));
    }
}
