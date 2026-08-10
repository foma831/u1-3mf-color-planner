use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Highest source filament state guaranteed by the analyzer.
pub const MAX_PAINT_STATE: u8 = 32;

/// One node in the recursive Bambu/Orca TriangleSelector annotation tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaintNode {
    Leaf {
        /// Zero means inherit the part/object extruder. Values 1-32 are source slots.
        state: u8,
    },
    Split {
        split_sides: u8,
        special_side: u8,
        /// Children are retained in serialized bitstream order.
        children: Vec<PaintNode>,
    },
}

impl PaintNode {
    pub fn used_states(&self) -> BTreeSet<u8> {
        let mut states = BTreeSet::new();
        self.collect_states(&mut states);
        states
    }

    fn collect_states(&self, states: &mut BTreeSet<u8>) {
        match self {
            Self::Leaf { state } => {
                states.insert(*state);
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.collect_states(states);
                }
            }
        }
    }

    /// Remap nonzero leaf filament states only.
    ///
    /// Inherit state 0, split descriptors, and child order are unchanged.
    pub fn remap_states(
        &mut self,
        mut remap: impl FnMut(u8) -> Result<u8, PaintCodecError>,
    ) -> Result<(), PaintCodecError> {
        let mut candidate = self.clone();
        candidate.remap_states_inner(&mut remap)?;
        *self = candidate;
        Ok(())
    }

    fn remap_states_inner<F>(&mut self, remap: &mut F) -> Result<(), PaintCodecError>
    where
        F: FnMut(u8) -> Result<u8, PaintCodecError>,
    {
        match self {
            Self::Leaf { state } => {
                if *state == 0 {
                    return Ok(());
                }
                let mapped = remap(*state)?;
                if mapped == 0 {
                    return Err(PaintCodecError::FilamentMappedToInherit);
                }
                if mapped > MAX_PAINT_STATE {
                    return Err(PaintCodecError::StateOutOfRange(mapped as u32));
                }
                *state = mapped;
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.remap_states_inner(remap)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PaintCodecError {
    #[error("paint annotation is empty")]
    Empty,
    #[error("invalid hexadecimal character {character:?} at byte {index}")]
    InvalidHex { character: char, index: usize },
    #[error("paint annotation ended before the tree was complete")]
    UnexpectedEnd,
    #[error("paint annotation has trailing tree data")]
    TrailingData,
    #[error("paint state {0} is outside the supported range 0-32")]
    StateOutOfRange(u32),
    #[error("a nonzero filament state cannot be remapped to inherit state 0")]
    FilamentMappedToInherit,
    #[error("paint split has {actual} children; expected {expected}")]
    InvalidChildCount { expected: usize, actual: usize },
    #[error("paint split_sides must be in 1-3, got {0}")]
    InvalidSplitSides(u8),
    #[error("paint special_side must be in 0-3, got {0}")]
    InvalidSpecialSide(u8),
    #[error("paint tree exceeds the safe nesting limit")]
    NestingLimit,
}

const MAX_TREE_DEPTH: usize = 64;

/// Decode a nibble-reversed TriangleSelector annotation.
pub fn decode_paint_annotation(encoded: &str) -> Result<PaintNode, PaintCodecError> {
    if encoded.is_empty() {
        return Err(PaintCodecError::Empty);
    }

    let mut nibbles = Vec::with_capacity(encoded.len());
    for (index, character) in encoded.char_indices().rev() {
        let value = character
            .to_digit(16)
            .ok_or(PaintCodecError::InvalidHex { character, index })?;
        nibbles.push(value as u8);
    }

    let mut cursor = 0;
    let tree = decode_node(&nibbles, &mut cursor, 0)?;
    if cursor != nibbles.len() {
        return Err(PaintCodecError::TrailingData);
    }
    Ok(tree)
}

fn decode_node(
    nibbles: &[u8],
    cursor: &mut usize,
    depth: usize,
) -> Result<PaintNode, PaintCodecError> {
    if depth > MAX_TREE_DEPTH {
        return Err(PaintCodecError::NestingLimit);
    }
    let code = *nibbles.get(*cursor).ok_or(PaintCodecError::UnexpectedEnd)?;
    *cursor += 1;

    let split_sides = code & 0b11;
    if split_sides == 0 {
        let prefix = code >> 2;
        let state = if prefix < 3 {
            u32::from(prefix)
        } else {
            let mut state = 3_u32;
            loop {
                let extension = *nibbles.get(*cursor).ok_or(PaintCodecError::UnexpectedEnd)?;
                *cursor += 1;
                state = state
                    .checked_add(u32::from(extension))
                    .ok_or(PaintCodecError::StateOutOfRange(u32::MAX))?;
                if state > u32::from(MAX_PAINT_STATE) {
                    return Err(PaintCodecError::StateOutOfRange(state));
                }
                if extension != 0x0f {
                    break;
                }
            }
            state
        };
        return Ok(PaintNode::Leaf { state: state as u8 });
    }

    let special_side = code >> 2;
    let child_count = usize::from(split_sides) + 1;
    let mut children = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        children.push(decode_node(nibbles, cursor, depth + 1)?);
    }
    Ok(PaintNode::Split {
        split_sides,
        special_side,
        children,
    })
}

/// Encode a TriangleSelector tree to canonical uppercase hexadecimal.
pub fn encode_paint_annotation(tree: &PaintNode) -> Result<String, PaintCodecError> {
    let mut nibbles = Vec::new();
    encode_node(tree, &mut nibbles, 0)?;
    let mut encoded = String::with_capacity(nibbles.len());
    for nibble in nibbles.into_iter().rev() {
        encoded.push(
            char::from_digit(u32::from(nibble), 16)
                .expect("nibble")
                .to_ascii_uppercase(),
        );
    }
    Ok(encoded)
}

fn encode_node(
    tree: &PaintNode,
    nibbles: &mut Vec<u8>,
    depth: usize,
) -> Result<(), PaintCodecError> {
    if depth > MAX_TREE_DEPTH {
        return Err(PaintCodecError::NestingLimit);
    }
    match tree {
        PaintNode::Leaf { state } => {
            if *state > MAX_PAINT_STATE {
                return Err(PaintCodecError::StateOutOfRange(u32::from(*state)));
            }
            if *state < 3 {
                nibbles.push(*state << 2);
            } else {
                nibbles.push(0x0c);
                let mut remaining = *state - 3;
                while remaining >= 15 {
                    nibbles.push(0x0f);
                    remaining -= 15;
                }
                nibbles.push(remaining);
            }
        }
        PaintNode::Split {
            split_sides,
            special_side,
            children,
        } => {
            if !(1..=3).contains(split_sides) {
                return Err(PaintCodecError::InvalidSplitSides(*split_sides));
            }
            if *special_side > 3 {
                return Err(PaintCodecError::InvalidSpecialSide(*special_side));
            }
            let expected = usize::from(*split_sides) + 1;
            if children.len() != expected {
                return Err(PaintCodecError::InvalidChildCount {
                    expected,
                    actual: children.len(),
                });
            }
            nibbles.push((*special_side << 2) | *split_sides);
            for child in children {
                encode_node(child, nibbles, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// Decode only the leaf states used by an annotation.
pub fn used_paint_states(encoded: &str) -> Result<BTreeSet<u8>, PaintCodecError> {
    Ok(decode_paint_annotation(encoded)?.used_states())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_supported_leaf_states_round_trip() {
        for state in 0..=MAX_PAINT_STATE {
            let tree = PaintNode::Leaf { state };
            let encoded = encode_paint_annotation(&tree).unwrap();
            assert_eq!(decode_paint_annotation(&encoded).unwrap(), tree);
            assert!(encoded.chars().all(|character| {
                character.is_ascii_digit() || ('A'..='F').contains(&character)
            }));
        }
    }

    #[test]
    fn known_leaf_encodings_match_triangle_selector_layout() {
        assert_eq!(
            encode_paint_annotation(&PaintNode::Leaf { state: 0 }).unwrap(),
            "0"
        );
        assert_eq!(
            encode_paint_annotation(&PaintNode::Leaf { state: 1 }).unwrap(),
            "4"
        );
        assert_eq!(
            encode_paint_annotation(&PaintNode::Leaf { state: 2 }).unwrap(),
            "8"
        );
        assert_eq!(
            encode_paint_annotation(&PaintNode::Leaf { state: 3 }).unwrap(),
            "0C"
        );
        assert_eq!(
            encode_paint_annotation(&PaintNode::Leaf { state: 18 }).unwrap(),
            "0FC"
        );
        assert_eq!(
            encode_paint_annotation(&PaintNode::Leaf { state: 32 }).unwrap(),
            "EFC"
        );
    }

    #[test]
    fn recursively_split_tree_round_trips_and_keeps_inherit() {
        let tree = PaintNode::Split {
            split_sides: 2,
            special_side: 1,
            children: vec![
                PaintNode::Leaf { state: 0 },
                PaintNode::Split {
                    split_sides: 1,
                    special_side: 2,
                    children: vec![PaintNode::Leaf { state: 32 }, PaintNode::Leaf { state: 7 }],
                },
                PaintNode::Leaf { state: 3 },
            ],
        };
        let encoded = encode_paint_annotation(&tree).unwrap();
        assert_eq!(decode_paint_annotation(&encoded).unwrap(), tree);
        assert_eq!(
            used_paint_states(&encoded).unwrap(),
            BTreeSet::from([0, 3, 7, 32])
        );
    }

    #[test]
    fn deterministic_property_style_trees_round_trip() {
        let mut seed = 0x6a09_e667_f3bc_c909_u64;
        for _ in 0..2_000 {
            let tree = generated_tree(&mut seed, 0);
            let encoded = encode_paint_annotation(&tree).unwrap();
            let decoded = decode_paint_annotation(&encoded).unwrap();
            assert_eq!(decoded, tree);
            assert_eq!(encode_paint_annotation(&decoded).unwrap(), encoded);
        }
    }

    fn generated_tree(seed: &mut u64, depth: usize) -> PaintNode {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        if depth >= 5 || (*seed & 3) != 0 {
            PaintNode::Leaf {
                state: (*seed % (u64::from(MAX_PAINT_STATE) + 1)) as u8,
            }
        } else {
            let split_sides = ((*seed >> 8) % 3 + 1) as u8;
            let special_side = ((*seed >> 16) % 4) as u8;
            let children = (0..=split_sides)
                .map(|_| generated_tree(seed, depth + 1))
                .collect();
            PaintNode::Split {
                split_sides,
                special_side,
                children,
            }
        }
    }

    #[test]
    fn remap_changes_leaf_states_without_touching_tree_shape() {
        let mut tree = PaintNode::Split {
            split_sides: 1,
            special_side: 2,
            children: vec![PaintNode::Leaf { state: 0 }, PaintNode::Leaf { state: 12 }],
        };
        let mut visited = Vec::new();
        tree.remap_states(|state| {
            visited.push(state);
            Ok(if state == 12 { 4 } else { state })
        })
        .unwrap();
        assert_eq!(visited, [12]);
        assert_eq!(tree.used_states(), BTreeSet::from([0, 4]));

        let before_failed_remap = tree.clone();
        assert_eq!(
            tree.remap_states(|_| Ok(0)),
            Err(PaintCodecError::FilamentMappedToInherit)
        );
        assert_eq!(tree, before_failed_remap);
    }

    #[test]
    fn malformed_data_is_rejected() {
        assert_eq!(decode_paint_annotation(""), Err(PaintCodecError::Empty));
        assert!(matches!(
            decode_paint_annotation("G"),
            Err(PaintCodecError::InvalidHex { .. })
        ));
        assert_eq!(
            decode_paint_annotation("C"),
            Err(PaintCodecError::UnexpectedEnd)
        );
        assert_eq!(
            decode_paint_annotation("00"),
            Err(PaintCodecError::TrailingData)
        );
    }
}
