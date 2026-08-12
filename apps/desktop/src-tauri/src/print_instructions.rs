use serde_json::{Map, Value};
use std::fmt::Write as _;

const INSTRUCTION_TITLE: &str = "U1 3MF PRINT INSTRUCTIONS";

fn object<'a>(value: &'a Value, description: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("The conversion manifest {description} must be an object."))
}

fn array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    description: &str,
) -> Result<&'a Vec<Value>, String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("The conversion manifest has no {description}."))
}

fn text<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    description: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("The conversion manifest has no {description}."))
}

fn optional_text(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(single_line)
        .filter(|value| !value.is_empty())
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_bullet(output: &mut String, value: &str) {
    writeln!(output, "- {}", single_line(value)).expect("writing to String cannot fail");
}

fn write_action_list(output: &mut String, actions: &[String], empty: &str) {
    if actions.is_empty() {
        write_bullet(output, empty);
        return;
    }
    for action in actions {
        write_bullet(output, action);
    }
}

fn write_loadout(output: &mut String, loadout: &[Value]) -> Result<(), String> {
    if loadout.is_empty() {
        write_bullet(
            output,
            "No physical spool loadout was declared. Stop and verify the project in its target slicer.",
        );
        return Ok(());
    }

    for value in loadout {
        let slot = object(value, "artifact loadout slot")?;
        let toolhead = single_line(text(slot, "toolhead", "loadout toolhead")?);
        let spool = optional_text(slot, "spoolName")
            .or_else(|| optional_text(slot, "spoolId"))
            .unwrap_or_else(|| "Unassigned spool".to_owned());
        let details = [
            optional_text(slot, "material"),
            optional_text(slot, "color"),
            optional_text(slot, "profile").map(|profile| format!("Profile {profile}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if details.is_empty() {
            writeln!(output, "- {toolhead}: {spool}").expect("writing to String cannot fail");
        } else {
            writeln!(output, "- {toolhead}: {spool} | {}", details.join(" | "))
                .expect("writing to String cannot fail");
        }
    }
    Ok(())
}

pub(crate) fn render_print_instructions(manifest: &Value) -> Result<Vec<u8>, String> {
    let root = object(manifest, "root")?;
    let source = root
        .get("source")
        .ok_or_else(|| "The conversion manifest has no source identity.".to_owned())?;
    let source = object(source, "source identity")?;
    let source_file_name = single_line(text(source, "fileName", "source file name")?);
    let artifacts = array(root, "artifacts", "artifact list")?;
    if artifacts.is_empty() {
        return Err("The conversion manifest has no printable artifacts.".to_owned());
    }
    let exclusions = array(root, "excludedSourceUnits", "exclusion list")?;
    let warnings = array(root, "warnings", "warning list")?;

    let mut output = String::with_capacity(16 * 1024);
    writeln!(output, "{INSTRUCTION_TITLE}").expect("writing to String cannot fail");
    writeln!(output, "{}", "=".repeat(INSTRUCTION_TITLE.len()))
        .expect("writing to String cannot fail");
    writeln!(output, "Source project: {source_file_name}").expect("writing to String cannot fail");
    writeln!(output, "Printable project files: {}", artifacts.len())
        .expect("writing to String cannot fail");
    output.push_str(
        "\nFollow the steps in this exact order. Finish the active print before changing any spool for the next step. Open every project in the stated slicer and verify the printer, nozzle, plate, process, and physical filament mapping before starting.\n\n",
    );

    for (index, value) in artifacts.iter().enumerate() {
        let artifact = object(value, "artifact")?;
        let file_name = single_line(text(artifact, "fileName", "artifact file name")?);
        let relative_path = single_line(text(artifact, "relativePath", "artifact relative path")?);
        let printer = single_line(text(artifact, "printer", "artifact printer")?);
        let slicer = single_line(text(artifact, "slicer", "artifact slicer")?);
        let strategy = single_line(text(artifact, "strategy", "artifact strategy")?);
        let batch_id = single_line(text(artifact, "batchId", "artifact batch identity")?);
        let target_plates = array(artifact, "targetPlateIds", "artifact target plate list")?
            .iter()
            .map(|plate| {
                plate
                    .as_str()
                    .map(single_line)
                    .ok_or_else(|| "A target plate identity is not text.".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let loadout = array(artifact, "loadout", "artifact loadout")?;
        let setup_actions = array(artifact, "setupActions", "artifact setup action list")?
            .iter()
            .map(|action| {
                action
                    .as_str()
                    .map(single_line)
                    .ok_or_else(|| "A setup action is not text.".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (after_actions, before_actions): (Vec<_>, Vec<_>) = setup_actions
            .into_iter()
            .partition(|action| action.starts_with("After batch:"));

        writeln!(
            output,
            "STEP {} OF {} — {file_name}",
            index + 1,
            artifacts.len()
        )
        .expect("writing to String cannot fail");
        writeln!(output, "{}", "-".repeat(72)).expect("writing to String cannot fail");
        writeln!(output, "Printer: {printer}").expect("writing to String cannot fail");
        writeln!(output, "Slicer: {slicer}").expect("writing to String cannot fail");
        writeln!(output, "Strategy: {strategy}").expect("writing to String cannot fail");
        writeln!(output, "Batch: {batch_id}").expect("writing to String cannot fail");
        writeln!(output, "Project file: {relative_path}").expect("writing to String cannot fail");
        writeln!(
            output,
            "Target plates: {}",
            if target_plates.is_empty() {
                "Not declared".to_owned()
            } else {
                target_plates.join(", ")
            }
        )
        .expect("writing to String cannot fail");

        output.push_str("\nREQUIRED PHYSICAL LOADOUT\n");
        write_loadout(&mut output, loadout)?;

        output.push_str("\nBEFORE STARTING THIS STEP\n");
        write_action_list(
            &mut output,
            &before_actions,
            "No unload/load transition is required. Confirm that the required physical loadout above is already installed.",
        );

        output.push_str("\nPRINT\n");
        write_bullet(&mut output, &format!("Open {relative_path} in {slicer}."));
        write_bullet(
            &mut output,
            &format!("Confirm that the selected printer is {printer}."),
        );
        write_bullet(
            &mut output,
            "Confirm that each slicer filament slot maps to the physical spool and toolhead shown above.",
        );
        if target_plates.len() > 1 {
            write_bullet(
                &mut output,
                &format!(
                    "Print the target plates from this project in this order: {}.",
                    target_plates.join(", ")
                ),
            );
        } else {
            write_bullet(
                &mut output,
                "Print the prepared target plate and wait for it to finish.",
            );
        }

        output.push_str("\nAFTER THIS STEP FINISHES\n");
        write_action_list(
            &mut output,
            &after_actions,
            "Leave the current loadout installed until the next step tells you what to change.",
        );
        output.push('\n');
    }

    output.push_str("FINAL REVIEW\n------------\n");
    if exclusions.is_empty() {
        write_bullet(
            &mut output,
            "No source units were excluded from this conversion.",
        );
    } else {
        write_bullet(
            &mut output,
            &format!(
                "{} source unit(s) were explicitly excluded. Review conversion-report.html before printing.",
                exclusions.len()
            ),
        );
    }
    if warnings.is_empty() {
        write_bullet(&mut output, "No bundle-level warnings were recorded.");
    } else {
        write_bullet(
            &mut output,
            &format!(
                "{} acknowledged bundle warning(s) are recorded in conversion-report.html.",
                warnings.len()
            ),
        );
    }
    write_bullet(
        &mut output,
        "Do not print a project if its slicer preview, filament mapping, or physical loadout differs from this instruction file.",
    );

    Ok(output.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Value {
        serde_json::json!({
            "source": { "fileName": "fixture.3mf" },
            "artifacts": [
                {
                    "fileName": "batch-001.3mf",
                    "relativePath": "u1-direct/batch-001.3mf",
                    "printer": "Snapmaker U1",
                    "slicer": "Snapmaker Orca",
                    "strategy": "Direct Spools",
                    "batchId": "batch-1",
                    "targetPlateIds": ["plate-1", "plate-2"],
                    "loadout": [
                        {
                            "toolhead": "T1",
                            "spoolName": "Black",
                            "spoolId": "black",
                            "material": "PLA",
                            "color": "#010101",
                            "profile": "Generic PLA"
                        }
                    ],
                    "setupActions": [
                        "Before batch: T1 — load Black",
                        "After batch: T1 — restore Cyan"
                    ]
                },
                {
                    "fileName": "a1-001.3mf",
                    "relativePath": "a1-mini/a1-001.3mf",
                    "printer": "Bambu Lab A1 mini",
                    "slicer": "Bambu Studio",
                    "strategy": "A1 Mono",
                    "batchId": "job-2",
                    "targetPlateIds": ["plate-3"],
                    "loadout": [
                        {
                            "toolhead": "External spool",
                            "spoolName": "Grey",
                            "material": "PETG",
                            "color": "#9199AA",
                            "profile": "Generic PETG"
                        }
                    ],
                    "setupActions": []
                }
            ],
            "excludedSourceUnits": [],
            "warnings": ["Verify toolhead mapping"]
        })
    }

    #[test]
    fn instructions_are_deterministic_and_follow_artifact_order() {
        let first = render_print_instructions(&manifest()).unwrap();
        let second = render_print_instructions(&manifest()).unwrap();
        assert_eq!(first, second);

        let text = String::from_utf8(first).unwrap();
        assert!(text.contains("STEP 1 OF 2 — batch-001.3mf"));
        assert!(text.contains("T1: Black | PLA | #010101 | Profile Generic PLA"));
        assert!(text.contains("Before batch: T1 — load Black"));
        assert!(text.contains("After batch: T1 — restore Cyan"));
        assert!(text.contains(
            "Print the target plates from this project in this order: plate-1, plate-2."
        ));
        assert!(text.contains("STEP 2 OF 2 — a1-001.3mf"));
        assert!(text.contains("External spool: Grey | PETG | #9199AA | Profile Generic PETG"));
        assert!(text.find("STEP 1 OF 2 — batch-001.3mf") < text.find("STEP 2 OF 2 — a1-001.3mf"));
    }

    #[test]
    fn instructions_reject_a_manifest_without_printable_artifacts() {
        let mut value = manifest();
        value["artifacts"] = serde_json::json!([]);

        let error = render_print_instructions(&value).unwrap_err();

        assert!(error.contains("no printable artifacts"));
    }

    #[test]
    fn instructions_collapse_untrusted_line_breaks() {
        let mut value = manifest();
        value["artifacts"][0]["loadout"][0]["spoolName"] =
            serde_json::json!("Black\nLOAD SOMETHING ELSE");

        let text = String::from_utf8(render_print_instructions(&value).unwrap()).unwrap();

        assert!(text.contains("T1: Black LOAD SOMETHING ELSE | PLA"));
        assert!(!text.contains("T1: Black\n"));
    }
}
