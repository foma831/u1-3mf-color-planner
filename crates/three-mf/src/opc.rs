use std::collections::{BTreeMap, BTreeSet};

use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use thiserror::Error;
use uuid::Uuid;

use crate::Transform3mf;

pub const CONTENT_TYPES_PATH: &str = "[Content_Types].xml";
pub const ROOT_RELATIONSHIPS_PATH: &str = "_rels/.rels";
pub const MAIN_MODEL_PATH: &str = "3D/3dmodel.model";
pub const MAIN_MODEL_RELATIONSHIPS_PATH: &str = "3D/_rels/3dmodel.model.rels";

pub const RELATIONSHIPS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-package.relationships+xml";
pub const MODEL_CONTENT_TYPE: &str = "application/vnd.ms-package.3dmanufacturing-3dmodel+xml";
pub const MODEL_RELATIONSHIP_TYPE: &str =
    "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";

const CONTENT_TYPES_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/content-types";
const RELATIONSHIPS_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships";
const CORE_MODEL_NAMESPACE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const PRODUCTION_NAMESPACE: &str =
    "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";

#[derive(Debug, Error)]
pub enum OpcBuildError {
    #[error("invalid OPC part path {path:?}: {reason}")]
    InvalidPartPath { path: String, reason: &'static str },
    #[error("invalid content-type extension {0:?}")]
    InvalidExtension(String),
    #[error("content-type default for extension {0:?} is duplicated")]
    DuplicateContentTypeDefault(String),
    #[error("content-type override for part {0:?} is duplicated")]
    DuplicateContentTypeOverride(String),
    #[error("content type must not be empty")]
    EmptyContentType,
    #[error("invalid relationship ID {0:?}")]
    InvalidRelationshipId(String),
    #[error("relationship type must not be empty")]
    EmptyRelationshipType,
    #[error("relationship {0:?} is duplicated")]
    DuplicateRelationshipId(String),
    #[error("object ID {0} is duplicated")]
    DuplicateObjectId(u32),
    #[error("build item UUID {0:?} is duplicated")]
    DuplicateBuildUuid(String),
    #[error("component UUID {0:?} is duplicated")]
    DuplicateComponentUuid(String),
    #[error("object UUID {0:?} is duplicated")]
    DuplicateObjectUuid(String),
    #[error("production UUID {0:?} is reused by more than one element")]
    DuplicateProductionUuid(String),
    #[error("invalid UUID {value:?}: {message}")]
    InvalidUuid { value: String, message: String },
    #[error("object and component IDs must be greater than zero")]
    ZeroObjectId,
    #[error("3MF resource ID {0} must be less than 2147483648")]
    ResourceIdOutOfRange(u32),
    #[error("production object {0} must contain at least one component")]
    EmptyProductionObject(u32),
    #[error("invalid 3MF model unit {0:?}")]
    InvalidModelUnit(String),
    #[error("model language must not be empty")]
    EmptyModelLanguage,
    #[error("build item references missing root object {0}")]
    MissingBuildObject(u32),
    #[error("production model requires at least one root object and one build item")]
    EmptyProductionModel,
    #[error("failed to serialize OPC XML: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("failed to write OPC XML: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentTypesBuilder {
    defaults: BTreeMap<String, String>,
    overrides: BTreeMap<String, String>,
}

impl ContentTypesBuilder {
    #[must_use]
    pub fn project_3mf() -> Self {
        let mut builder = Self::default();
        builder
            .defaults
            .insert("rels".to_owned(), RELATIONSHIPS_CONTENT_TYPE.to_owned());
        builder
            .defaults
            .insert("model".to_owned(), MODEL_CONTENT_TYPE.to_owned());
        builder
    }

    pub fn add_default(
        &mut self,
        extension: impl Into<String>,
        content_type: impl Into<String>,
    ) -> Result<(), OpcBuildError> {
        let extension = extension.into();
        validate_extension(&extension)?;
        let content_type = content_type.into();
        validate_content_type(&content_type)?;
        let extension = extension.to_ascii_lowercase();
        if self.defaults.contains_key(&extension) {
            return Err(OpcBuildError::DuplicateContentTypeDefault(extension));
        }
        self.defaults.insert(extension, content_type);
        Ok(())
    }

    pub fn add_override(
        &mut self,
        part_name: impl Into<String>,
        content_type: impl Into<String>,
    ) -> Result<(), OpcBuildError> {
        let part_name = canonical_absolute_part_name(&part_name.into())?;
        let content_type = content_type.into();
        validate_content_type(&content_type)?;
        if self.overrides.contains_key(&part_name) {
            return Err(OpcBuildError::DuplicateContentTypeOverride(part_name));
        }
        self.overrides.insert(part_name, content_type);
        Ok(())
    }

    pub fn to_xml(&self) -> Result<Vec<u8>, OpcBuildError> {
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 1);
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        let mut root = BytesStart::new("Types");
        root.push_attribute(("xmlns", CONTENT_TYPES_NAMESPACE));
        writer.write_event(Event::Start(root))?;
        for (extension, content_type) in &self.defaults {
            let mut element = BytesStart::new("Default");
            element.push_attribute(("Extension", extension.as_str()));
            element.push_attribute(("ContentType", content_type.as_str()));
            writer.write_event(Event::Empty(element))?;
        }
        for (part_name, content_type) in &self.overrides {
            let mut element = BytesStart::new("Override");
            element.push_attribute(("PartName", part_name.as_str()));
            element.push_attribute(("ContentType", content_type.as_str()));
            writer.write_event(Event::Empty(element))?;
        }
        writer.write_event(Event::End(BytesEnd::new("Types")))?;
        Ok(writer.into_inner())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RelationshipTargetMode {
    #[default]
    Internal,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpcRelationship {
    pub id: String,
    pub relationship_type: String,
    pub target: String,
    pub target_mode: RelationshipTargetMode,
}

impl OpcRelationship {
    pub fn internal(
        id: impl Into<String>,
        relationship_type: impl Into<String>,
        target: impl Into<String>,
    ) -> Result<Self, OpcBuildError> {
        let relationship = Self {
            id: id.into(),
            relationship_type: relationship_type.into(),
            target: canonical_absolute_part_name(&target.into())?,
            target_mode: RelationshipTargetMode::Internal,
        };
        relationship.validate()?;
        Ok(relationship)
    }

    pub fn external(
        id: impl Into<String>,
        relationship_type: impl Into<String>,
        target: impl Into<String>,
    ) -> Result<Self, OpcBuildError> {
        let relationship = Self {
            id: id.into(),
            relationship_type: relationship_type.into(),
            target: target.into(),
            target_mode: RelationshipTargetMode::External,
        };
        relationship.validate()?;
        Ok(relationship)
    }

    fn validate(&self) -> Result<(), OpcBuildError> {
        if !is_portable_relationship_id(&self.id) {
            return Err(OpcBuildError::InvalidRelationshipId(self.id.clone()));
        }
        if self.relationship_type.is_empty() {
            return Err(OpcBuildError::EmptyRelationshipType);
        }
        if self.target.is_empty() || self.target.contains('\0') {
            return Err(OpcBuildError::InvalidPartPath {
                path: self.target.clone(),
                reason: "target is empty or contains NUL",
            });
        }
        if self.target_mode == RelationshipTargetMode::Internal {
            canonical_absolute_part_name(&self.target)?;
        }
        Ok(())
    }
}

pub fn relationships_xml(relationships: &[OpcRelationship]) -> Result<Vec<u8>, OpcBuildError> {
    let mut ordered = relationships.to_vec();
    ordered.sort_by(|left, right| left.id.cmp(&right.id));
    let mut ids = BTreeSet::new();
    for relationship in &ordered {
        relationship.validate()?;
        if !ids.insert(relationship.id.as_str()) {
            return Err(OpcBuildError::DuplicateRelationshipId(
                relationship.id.clone(),
            ));
        }
    }

    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 1);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    let mut root = BytesStart::new("Relationships");
    root.push_attribute(("xmlns", RELATIONSHIPS_NAMESPACE));
    writer.write_event(Event::Start(root))?;
    for relationship in ordered {
        let mut element = BytesStart::new("Relationship");
        element.push_attribute(("Target", relationship.target.as_str()));
        element.push_attribute(("Id", relationship.id.as_str()));
        element.push_attribute(("Type", relationship.relationship_type.as_str()));
        if relationship.target_mode == RelationshipTargetMode::External {
            element.push_attribute(("TargetMode", "External"));
        }
        writer.write_event(Event::Empty(element))?;
    }
    writer.write_event(Event::End(BytesEnd::new("Relationships")))?;
    Ok(writer.into_inner())
}

pub fn relationship_part_path(source_part: Option<&str>) -> Result<String, OpcBuildError> {
    let Some(source_part) = source_part else {
        return Ok(ROOT_RELATIONSHIPS_PATH.to_owned());
    };
    let source_part = canonical_relative_part_name(source_part)?;
    let (directory, file_name) = source_part
        .rsplit_once('/')
        .map_or(("", source_part.as_str()), |(directory, file_name)| {
            (directory, file_name)
        });
    Ok(if directory.is_empty() {
        format!("_rels/{file_name}.rels")
    } else {
        format!("{directory}/_rels/{file_name}.rels")
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProductionComponent {
    pub path: String,
    pub object_id: u32,
    pub uuid: String,
    pub transform: Option<Transform3mf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProductionObject {
    pub id: u32,
    pub uuid: String,
    pub name: Option<String>,
    pub components: Vec<ProductionComponent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProductionBuildItem {
    pub object_id: u32,
    pub uuid: String,
    pub transform: Option<Transform3mf>,
    pub printable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProductionModel {
    pub unit: String,
    pub language: String,
    pub build_uuid: String,
    pub metadata: BTreeMap<String, String>,
    pub objects: Vec<ProductionObject>,
    pub build_items: Vec<ProductionBuildItem>,
}

impl ProductionModel {
    /// Creates a production root model with a caller-owned build UUID.
    ///
    /// Production adapters should derive this UUID deterministically from the
    /// immutable source identity and canonical job ID. A fixed library default
    /// would violate cross-package uniqueness, while a random default would
    /// make otherwise identical packages non-reproducible.
    pub fn new(build_uuid: impl Into<String>) -> Self {
        Self {
            unit: "millimeter".to_owned(),
            language: "en-US".to_owned(),
            build_uuid: build_uuid.into(),
            metadata: BTreeMap::new(),
            objects: Vec::new(),
            build_items: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), OpcBuildError> {
        if self.objects.is_empty() || self.build_items.is_empty() {
            return Err(OpcBuildError::EmptyProductionModel);
        }
        if !matches!(
            self.unit.as_str(),
            "micron" | "millimeter" | "centimeter" | "inch" | "foot" | "meter"
        ) {
            return Err(OpcBuildError::InvalidModelUnit(self.unit.clone()));
        }
        if self.language.trim().is_empty() {
            return Err(OpcBuildError::EmptyModelLanguage);
        }
        let mut object_ids = BTreeSet::new();
        let mut object_uuids = BTreeSet::new();
        let mut component_uuids = BTreeSet::new();
        let mut build_uuids = BTreeSet::new();
        let mut all_uuids = BTreeSet::new();
        validate_and_insert_uuid(&self.build_uuid, &mut all_uuids)?;
        for object in &self.objects {
            if object.id == 0 {
                return Err(OpcBuildError::ZeroObjectId);
            }
            validate_resource_id(object.id)?;
            if object.components.is_empty() {
                return Err(OpcBuildError::EmptyProductionObject(object.id));
            }
            if !object_ids.insert(object.id) {
                return Err(OpcBuildError::DuplicateObjectId(object.id));
            }
            validate_uuid(&object.uuid)?;
            if !object_uuids.insert(object.uuid.as_str()) {
                return Err(OpcBuildError::DuplicateObjectUuid(object.uuid.clone()));
            }
            validate_and_insert_uuid(&object.uuid, &mut all_uuids)?;
            for component in &object.components {
                if component.object_id == 0 {
                    return Err(OpcBuildError::ZeroObjectId);
                }
                validate_resource_id(component.object_id)?;
                let canonical_path = canonical_absolute_part_name(&component.path)?;
                if canonical_path != component.path {
                    return Err(OpcBuildError::InvalidPartPath {
                        path: component.path.clone(),
                        reason: "Production p:path must be an absolute canonical package path",
                    });
                }
                if canonical_path == format!("/{MAIN_MODEL_PATH}") {
                    return Err(OpcBuildError::InvalidPartPath {
                        path: component.path.clone(),
                        reason: "Production p:path cannot target the root model part itself",
                    });
                }
                validate_uuid(&component.uuid)?;
                if !component_uuids.insert(component.uuid.as_str()) {
                    return Err(OpcBuildError::DuplicateComponentUuid(
                        component.uuid.clone(),
                    ));
                }
                validate_and_insert_uuid(&component.uuid, &mut all_uuids)?;
                if component
                    .transform
                    .is_some_and(|transform| !transform.is_finite())
                {
                    return Err(OpcBuildError::InvalidPartPath {
                        path: component.path.clone(),
                        reason: "component transform is not finite",
                    });
                }
            }
        }
        for item in &self.build_items {
            if item.object_id == 0 {
                return Err(OpcBuildError::ZeroObjectId);
            }
            validate_resource_id(item.object_id)?;
            if !object_ids.contains(&item.object_id) {
                return Err(OpcBuildError::MissingBuildObject(item.object_id));
            }
            validate_uuid(&item.uuid)?;
            if !build_uuids.insert(item.uuid.as_str()) {
                return Err(OpcBuildError::DuplicateBuildUuid(item.uuid.clone()));
            }
            validate_and_insert_uuid(&item.uuid, &mut all_uuids)?;
            if item
                .transform
                .is_some_and(|transform| !transform.is_finite())
            {
                return Err(OpcBuildError::InvalidPartPath {
                    path: item.object_id.to_string(),
                    reason: "build transform is not finite",
                });
            }
        }
        Ok(())
    }

    pub fn relationship_entries(&self) -> Result<Vec<OpcRelationship>, OpcBuildError> {
        self.validate()?;
        let paths = self
            .objects
            .iter()
            .flat_map(|object| object.components.iter())
            .map(|component| canonical_absolute_part_name(&component.path))
            .collect::<Result<BTreeSet<_>, _>>()?;
        paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                OpcRelationship::internal(
                    format!("rel-{}", index + 1),
                    MODEL_RELATIONSHIP_TYPE,
                    path,
                )
            })
            .collect()
    }

    pub fn to_xml(&self) -> Result<Vec<u8>, OpcBuildError> {
        self.validate()?;
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 1);
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
        let mut model = BytesStart::new("model");
        model.push_attribute(("xmlns", CORE_MODEL_NAMESPACE));
        model.push_attribute(("xmlns:p", PRODUCTION_NAMESPACE));
        model.push_attribute(("unit", self.unit.as_str()));
        model.push_attribute(("xml:lang", self.language.as_str()));
        model.push_attribute(("requiredextensions", "p"));
        writer.write_event(Event::Start(model))?;

        for (name, value) in &self.metadata {
            let mut metadata = BytesStart::new("metadata");
            metadata.push_attribute(("name", name.as_str()));
            writer.write_event(Event::Start(metadata))?;
            writer.write_event(Event::Text(BytesText::new(value)))?;
            writer.write_event(Event::End(BytesEnd::new("metadata")))?;
        }

        writer.write_event(Event::Start(BytesStart::new("resources")))?;
        let mut objects = self.objects.iter().collect::<Vec<_>>();
        objects.sort_by_key(|object| object.id);
        for object in objects {
            let mut object_element = BytesStart::new("object");
            let id = object.id.to_string();
            object_element.push_attribute(("id", id.as_str()));
            object_element.push_attribute(("p:UUID", object.uuid.as_str()));
            object_element.push_attribute(("type", "model"));
            if let Some(name) = &object.name {
                object_element.push_attribute(("name", name.as_str()));
            }
            writer.write_event(Event::Start(object_element))?;
            writer.write_event(Event::Start(BytesStart::new("components")))?;
            for component in &object.components {
                let mut component_element = BytesStart::new("component");
                let object_id = component.object_id.to_string();
                component_element.push_attribute(("p:path", component.path.as_str()));
                component_element.push_attribute(("objectid", object_id.as_str()));
                component_element.push_attribute(("p:UUID", component.uuid.as_str()));
                let transform = component.transform.map(format_transform);
                if let Some(transform) = transform.as_deref() {
                    component_element.push_attribute(("transform", transform));
                }
                writer.write_event(Event::Empty(component_element))?;
            }
            writer.write_event(Event::End(BytesEnd::new("components")))?;
            writer.write_event(Event::End(BytesEnd::new("object")))?;
        }
        writer.write_event(Event::End(BytesEnd::new("resources")))?;

        let mut build = BytesStart::new("build");
        build.push_attribute(("p:UUID", self.build_uuid.as_str()));
        writer.write_event(Event::Start(build))?;
        for item in &self.build_items {
            let mut item_element = BytesStart::new("item");
            let object_id = item.object_id.to_string();
            item_element.push_attribute(("objectid", object_id.as_str()));
            item_element.push_attribute(("p:UUID", item.uuid.as_str()));
            let printable = if item.printable { "1" } else { "0" };
            item_element.push_attribute(("printable", printable));
            let transform = item.transform.map(format_transform);
            if let Some(transform) = transform.as_deref() {
                item_element.push_attribute(("transform", transform));
            }
            writer.write_event(Event::Empty(item_element))?;
        }
        writer.write_event(Event::End(BytesEnd::new("build")))?;
        writer.write_event(Event::End(BytesEnd::new("model")))?;
        Ok(writer.into_inner())
    }
}

fn validate_extension(extension: &str) -> Result<(), OpcBuildError> {
    if extension.is_empty()
        || extension.starts_with('.')
        || extension
            .chars()
            .any(|character| !character.is_ascii_alphanumeric())
    {
        return Err(OpcBuildError::InvalidExtension(extension.to_owned()));
    }
    Ok(())
}

fn validate_content_type(content_type: &str) -> Result<(), OpcBuildError> {
    if content_type.trim().is_empty() {
        return Err(OpcBuildError::EmptyContentType);
    }
    Ok(())
}

fn canonical_relative_part_name(path: &str) -> Result<String, OpcBuildError> {
    let path = path.strip_prefix('/').unwrap_or(path);
    if path.is_empty() {
        return Err(OpcBuildError::InvalidPartPath {
            path: path.to_owned(),
            reason: "path is empty",
        });
    }
    if path.contains('\0')
        || path.contains('\\')
        || path.contains('?')
        || path.contains('#')
        || path.contains('%')
        || path.chars().any(char::is_control)
    {
        return Err(OpcBuildError::InvalidPartPath {
            path: path.to_owned(),
            reason: "path contains a control character, query, fragment, percent escape, or backslash",
        });
    }
    if path.split('/').any(|component| {
        component.is_empty() || component == "." || component == ".." || component.contains(':')
    }) {
        return Err(OpcBuildError::InvalidPartPath {
            path: path.to_owned(),
            reason: "path contains an empty, current, or parent component",
        });
    }
    Ok(path.to_owned())
}

fn canonical_absolute_part_name(path: &str) -> Result<String, OpcBuildError> {
    Ok(format!("/{}", canonical_relative_part_name(path)?))
}

fn validate_uuid(value: &str) -> Result<(), OpcBuildError> {
    let parsed = Uuid::parse_str(value).map_err(|error| OpcBuildError::InvalidUuid {
        value: value.to_owned(),
        message: error.to_string(),
    })?;
    if parsed.hyphenated().to_string() != value {
        return Err(OpcBuildError::InvalidUuid {
            value: value.to_owned(),
            message: "UUID must use canonical lowercase hyphenated form".to_owned(),
        });
    }
    Ok(())
}

fn validate_resource_id(value: u32) -> Result<(), OpcBuildError> {
    if value >= 2_147_483_648 {
        return Err(OpcBuildError::ResourceIdOutOfRange(value));
    }
    Ok(())
}

fn is_portable_relationship_id(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn validate_and_insert_uuid<'a>(
    value: &'a str,
    uuids: &mut BTreeSet<&'a str>,
) -> Result<(), OpcBuildError> {
    validate_uuid(value)?;
    if !uuids.insert(value) {
        return Err(OpcBuildError::DuplicateProductionUuid(value.to_owned()));
    }
    Ok(())
}

fn format_transform(transform: Transform3mf) -> String {
    transform
        .values
        .iter()
        .map(|value| format!("{value:.15}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::Reader;
    use quick_xml::events::Event;

    fn assert_well_formed(xml: &[u8]) {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(error) => panic!("generated XML is invalid: {error}"),
            }
            buffer.clear();
        }
    }

    #[test]
    fn content_types_are_deterministic_and_well_formed() {
        let mut builder = ContentTypesBuilder::project_3mf();
        builder
            .add_default("png", "image/png")
            .expect("valid PNG type");
        builder
            .add_override("/Metadata/project_settings.config", "application/json")
            .expect("valid override");

        let first = builder.to_xml().expect("serialize content types");
        let second = builder.to_xml().expect("serialize content types");
        assert_eq!(first, second);
        assert_well_formed(&first);
        let text = String::from_utf8(first).expect("UTF-8 XML");
        assert!(text.contains("Extension=\"model\""));
        assert!(text.contains("PartName=\"/Metadata/project_settings.config\""));

        assert!(matches!(
            builder.add_default("PNG", "image/png"),
            Err(OpcBuildError::DuplicateContentTypeDefault(extension)) if extension == "png"
        ));
        assert!(matches!(
            builder.add_override("Metadata/project_settings.config", "application/json"),
            Err(OpcBuildError::DuplicateContentTypeOverride(path))
                if path == "/Metadata/project_settings.config"
        ));
    }

    #[test]
    fn relationships_are_sorted_and_reject_duplicates() {
        let model = OpcRelationship::internal("rel-2", MODEL_RELATIONSHIP_TYPE, MAIN_MODEL_PATH)
            .expect("valid model relationship");
        let external = OpcRelationship::external(
            "rel-1",
            "https://example.invalid/type",
            "https://example.invalid/asset",
        )
        .expect("valid external relationship");
        let xml = relationships_xml(&[model.clone(), external]).expect("serialize relationships");
        assert_well_formed(&xml);
        let text = String::from_utf8(xml).expect("UTF-8 XML");
        assert!(text.find("rel-1").unwrap() < text.find("rel-2").unwrap());

        let error = relationships_xml(&[model.clone(), model])
            .expect_err("duplicate relationship IDs must fail");
        assert!(matches!(error, OpcBuildError::DuplicateRelationshipId(_)));
        assert!(matches!(
            OpcRelationship::internal("invalid id", MODEL_RELATIONSHIP_TYPE, MAIN_MODEL_PATH),
            Err(OpcBuildError::InvalidRelationshipId(_))
        ));
    }

    #[test]
    fn relationship_part_paths_follow_opc_convention() {
        assert_eq!(
            relationship_part_path(None).expect("root relationship path"),
            ROOT_RELATIONSHIPS_PATH
        );
        assert_eq!(
            relationship_part_path(Some(MAIN_MODEL_PATH)).expect("model relationship path"),
            MAIN_MODEL_RELATIONSHIPS_PATH
        );
    }

    #[test]
    fn production_model_emits_external_components_and_build() {
        let mut model = ProductionModel::new("00000000-0000-4000-8000-000000000001");
        model
            .metadata
            .insert("Application".to_owned(), "U1 3MF Color Planner".to_owned());
        model.objects.push(ProductionObject {
            id: 2,
            uuid: "00000000-0000-4000-8000-000000000002".to_owned(),
            name: Some("Fixture".to_owned()),
            components: vec![ProductionComponent {
                path: "/3D/Objects/fixture.model".to_owned(),
                object_id: 1,
                uuid: "00000000-0000-4000-8000-000000000003".to_owned(),
                transform: Some(Transform3mf::IDENTITY),
            }],
        });
        model.build_items.push(ProductionBuildItem {
            object_id: 2,
            uuid: "00000000-0000-4000-8000-000000000004".to_owned(),
            transform: Some(Transform3mf::IDENTITY),
            printable: true,
        });

        let xml = model.to_xml().expect("serialize production model");
        assert_well_formed(&xml);
        let text = String::from_utf8(xml).expect("UTF-8 XML");
        assert!(text.contains("requiredextensions=\"p\""));
        assert!(text.contains("p:path=\"/3D/Objects/fixture.model\""));
        assert!(text.contains("objectid=\"2\""));

        let relationships = model
            .relationship_entries()
            .expect("derive external model relationships");
        assert_eq!(relationships.len(), 1);
        assert_eq!(relationships[0].target, "/3D/Objects/fixture.model");
    }

    #[test]
    fn production_model_rejects_missing_build_object_and_unsafe_path() {
        let mut model = ProductionModel::new("00000000-0000-4000-8000-000000000001");
        model.objects.push(ProductionObject {
            id: 1,
            uuid: "00000000-0000-4000-8000-000000000004".to_owned(),
            name: None,
            components: vec![ProductionComponent {
                path: "/3D/../secret.model".to_owned(),
                object_id: 1,
                uuid: "00000000-0000-4000-8000-000000000002".to_owned(),
                transform: None,
            }],
        });
        model.build_items.push(ProductionBuildItem {
            object_id: 9,
            uuid: "00000000-0000-4000-8000-000000000003".to_owned(),
            transform: None,
            printable: true,
        });

        assert!(matches!(
            model.validate(),
            Err(OpcBuildError::InvalidPartPath { .. })
        ));

        model.objects[0].components[0].path = "3D/Objects/part.model".to_owned();
        assert!(matches!(
            model.validate(),
            Err(OpcBuildError::InvalidPartPath { .. })
        ));

        model.objects[0].components[0].path = format!("/{MAIN_MODEL_PATH}");
        assert!(matches!(
            model.validate(),
            Err(OpcBuildError::InvalidPartPath { .. })
        ));
    }

    #[test]
    fn production_model_rejects_noncanonical_uuids_empty_components_and_large_ids() {
        let mut model = ProductionModel::new("00000000-0000-4000-8000-000000000001");
        model.objects.push(ProductionObject {
            id: 1,
            uuid: "00000000-0000-4000-8000-00000000000A".to_owned(),
            name: None,
            components: vec![ProductionComponent {
                path: "/3D/Objects/part.model".to_owned(),
                object_id: 1,
                uuid: "00000000-0000-4000-8000-000000000002".to_owned(),
                transform: None,
            }],
        });
        model.build_items.push(ProductionBuildItem {
            object_id: 1,
            uuid: "00000000-0000-4000-8000-000000000003".to_owned(),
            transform: None,
            printable: true,
        });
        assert!(matches!(
            model.validate(),
            Err(OpcBuildError::InvalidUuid { .. })
        ));

        model.objects[0].uuid = "00000000-0000-4000-8000-00000000000a".to_owned();
        model.objects[0].components.clear();
        assert!(matches!(
            model.validate(),
            Err(OpcBuildError::EmptyProductionObject(1))
        ));

        model.objects[0].id = 2_147_483_648;
        assert!(matches!(
            model.validate(),
            Err(OpcBuildError::ResourceIdOutOfRange(2_147_483_648))
        ));
    }
}
