use serde_json::Value;

const REPORT_TITLE: &str = "U1 3MF Conversion Report";

pub(crate) fn render_conversion_report(manifest: &Value) -> Result<Vec<u8>, String> {
    let root = manifest
        .as_object()
        .ok_or_else(|| "The conversion manifest must be a JSON object.".to_owned())?;
    let source = root
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| "The conversion manifest has no source identity.".to_owned())?;
    let source_sha256 = source
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| "The conversion manifest source has no SHA-256 identity.".to_owned())?;
    let source_file_name = source
        .get("fileName")
        .and_then(Value::as_str)
        .ok_or_else(|| "The conversion manifest source has no file name.".to_owned())?;
    let source_application = source
        .get("application")
        .and_then(Value::as_str)
        .ok_or_else(|| "The conversion manifest source has no application identity.".to_owned())?;
    let source_application_version = source
        .get("applicationVersion")
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let source_dialect = source
        .get("dialect")
        .and_then(Value::as_str)
        .ok_or_else(|| "The conversion manifest source has no dialect identity.".to_owned())?;
    let converter = root
        .get("converter")
        .and_then(Value::as_object)
        .ok_or_else(|| "The conversion manifest has no converter identity.".to_owned())?;
    let build_identity = converter
        .get("buildIdentity")
        .and_then(Value::as_object)
        .ok_or_else(|| "The conversion manifest has no converter build identity.".to_owned())?;
    let converter_package_version = build_identity
        .get("packageVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| "The converter build identity has no package version.".to_owned())?;
    let converter_git_identity = build_identity
        .get("gitIdentity")
        .and_then(Value::as_str)
        .ok_or_else(|| "The converter build identity has no git identity.".to_owned())?;
    let plan_fingerprint = root
        .get("planFingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| "The conversion manifest has no plan fingerprint.".to_owned())?;
    let validation_status = root
        .get("validationStatus")
        .and_then(Value::as_str)
        .ok_or_else(|| "The conversion manifest has no validation status.".to_owned())?;
    let artifacts = root
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| "The conversion manifest has no artifact list.".to_owned())?;
    let mappings = root
        .get("sourceToTargetMap")
        .and_then(Value::as_array)
        .ok_or_else(|| "The conversion manifest has no source-to-target map.".to_owned())?;
    let warnings = root
        .get("warnings")
        .and_then(Value::as_array)
        .ok_or_else(|| "The conversion manifest has no warning list.".to_owned())?;
    let excluded_source_units = root
        .get("excludedSourceUnits")
        .and_then(Value::as_array)
        .ok_or_else(|| "The conversion manifest has no exclusion list.".to_owned())?;
    let warnings_acknowledged = root
        .get("warningsAcknowledged")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            "The conversion manifest has no warning acknowledgement state.".to_owned()
        })?;
    let acknowledged_warnings = root
        .get("acknowledgedWarnings")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "The conversion manifest has no exact warning acknowledgement evidence.".to_owned()
        })?;
    if acknowledged_warnings != warnings
        || warnings_acknowledged != !acknowledged_warnings.is_empty()
    {
        return Err(
            "The conversion manifest warning acknowledgement does not match its exact warning list."
                .into(),
        );
    }

    let mut html = String::with_capacity(32 * 1024);
    html.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n");
    html.push_str(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:\">\n",
    );
    html.push_str(
        "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>",
    );
    html.push_str(REPORT_TITLE);
    html.push_str("</title><style>");
    html.push_str(
        "body{font:14px system-ui,-apple-system,sans-serif;color:#20262d;background:#f5f7f8;margin:0}main{max-width:1180px;margin:auto;padding:32px}h1,h2{color:#151a1f}section{background:#fff;border:1px solid #d8dee4;border-radius:10px;padding:20px;margin:16px 0}dl{display:grid;grid-template-columns:max-content 1fr;gap:8px 18px}dt{font-weight:700}dd{margin:0;overflow-wrap:anywhere}table{width:100%;border-collapse:collapse}th,td{text-align:left;vertical-align:top;border-bottom:1px solid #e4e8eb;padding:8px}th{background:#eef2f4}code,pre{font:12px ui-monospace,SFMono-Regular,monospace;overflow-wrap:anywhere}pre{white-space:pre-wrap;background:#f3f5f6;padding:12px;border-radius:6px}.status{font-weight:700;color:#08753d}.warning{color:#8b5700}ul{padding-left:20px}.empty{color:#64717d}caption{text-align:left;font-weight:700;margin-bottom:8px}",
    );
    html.push_str("</style></head><body><main><header><h1>");
    html.push_str(REPORT_TITLE);
    html.push_str("</h1><p>This offline report is generated from the same validated manifest that accompanies the converted project files.</p></header>");

    html.push_str(
        "<section aria-labelledby=\"summary-heading\"><h2 id=\"summary-heading\">Summary</h2><dl>",
    );
    report_field(&mut html, "Source file", source_file_name);
    report_field(&mut html, "Source SHA-256", source_sha256);
    report_field(&mut html, "Source application", source_application);
    report_field(
        &mut html,
        "Source application version",
        source_application_version,
    );
    report_field(&mut html, "Source dialect", source_dialect);
    report_field(
        &mut html,
        "Converter package version",
        converter_package_version,
    );
    report_field(&mut html, "Converter git identity", converter_git_identity);
    report_field(&mut html, "Plan fingerprint", plan_fingerprint);
    html.push_str("<dt>Validation</dt><dd class=\"status\">");
    escape_html_into(&mut html, validation_status);
    html.push_str("</dd><dt>Output files</dt><dd>");
    html.push_str(&artifacts.len().to_string());
    html.push_str("</dd><dt>Mapped source units</dt><dd>");
    html.push_str(&mappings.len().to_string());
    html.push_str("</dd><dt>Explicitly excluded source units</dt><dd>");
    html.push_str(&excluded_source_units.len().to_string());
    html.push_str("</dd><dt>Warnings acknowledged</dt><dd>");
    html.push_str(if warnings_acknowledged { "Yes" } else { "No" });
    html.push_str("</dd></dl></section>");

    html.push_str("<section aria-labelledby=\"artifacts-heading\"><h2 id=\"artifacts-heading\">Output projects</h2><table><thead><tr><th>File</th><th>Printer / strategy</th><th>Plates</th><th>Physical loadout</th><th>SHA-256</th><th>Status</th></tr></thead><tbody>");
    for artifact in artifacts {
        let object = artifact
            .as_object()
            .ok_or_else(|| "A conversion artifact is not a JSON object.".to_owned())?;
        html.push_str("<tr>");
        report_cell(&mut html, required_string(object, "fileName", "artifact")?);
        let printer = required_string(object, "printer", "artifact")?;
        let strategy = required_string(object, "strategy", "artifact")?;
        html.push_str("<td>");
        escape_html_into(&mut html, printer);
        html.push_str("<br><small>");
        escape_html_into(&mut html, strategy);
        html.push_str("</small></td>");
        html.push_str("<td>");
        html.push_str(
            &object
                .get("plateCount")
                .and_then(Value::as_u64)
                .ok_or_else(|| "A conversion artifact has no plate count.".to_owned())?
                .to_string(),
        );
        html.push_str("</td><td>");
        let loadout = object
            .get("loadout")
            .and_then(Value::as_array)
            .ok_or_else(|| "A conversion artifact has no physical loadout.".to_owned())?;
        if loadout.is_empty() {
            html.push_str("<span class=\"empty\">No physical slots</span>");
        } else {
            html.push_str("<ul>");
            for slot in loadout {
                let slot = slot
                    .as_object()
                    .ok_or_else(|| "A physical loadout entry is not an object.".to_owned())?;
                html.push_str("<li>");
                escape_html_into(
                    &mut html,
                    slot.get("toolhead")
                        .and_then(Value::as_str)
                        .unwrap_or("Slot"),
                );
                html.push_str(": ");
                let spool = slot
                    .get("spoolName")
                    .and_then(Value::as_str)
                    .or_else(|| slot.get("spoolId").and_then(Value::as_str))
                    .unwrap_or("Unassigned");
                escape_html_into(&mut html, spool);
                if let Some(material) = slot.get("material").and_then(Value::as_str) {
                    html.push_str(" · ");
                    escape_html_into(&mut html, material);
                }
                html.push_str("</li>");
            }
            html.push_str("</ul>");
        }
        html.push_str("</td>");
        report_cell(&mut html, required_string(object, "sha256", "artifact")?);
        report_cell(
            &mut html,
            required_string(object, "validationStatus", "artifact")?,
        );
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table></section>");

    html.push_str("<section aria-labelledby=\"mapping-heading\"><h2 id=\"mapping-heading\">Source to target mapping</h2><table><thead><tr><th>Source unit</th><th>Output file</th><th>Target plate</th><th>Printer</th><th>Strategy</th></tr></thead><tbody>");
    for mapping in mappings {
        let object = mapping
            .as_object()
            .ok_or_else(|| "A source-to-target entry is not a JSON object.".to_owned())?;
        html.push_str("<tr>");
        for key in [
            "sourceUnitId",
            "outputFile",
            "targetPlateId",
            "printer",
            "strategy",
        ] {
            report_cell(&mut html, required_string(object, key, "source mapping")?);
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table></section>");

    html.push_str(
        "<section aria-labelledby=\"warnings-heading\"><h2 id=\"warnings-heading\">Warnings</h2>",
    );
    if warnings.is_empty() {
        html.push_str("<p class=\"empty\">No conversion warnings.</p>");
    } else {
        html.push_str("<ul class=\"warning\">");
        for warning in warnings {
            let warning = warning
                .as_str()
                .ok_or_else(|| "A conversion warning is not text.".to_owned())?;
            html.push_str("<li>");
            escape_html_into(&mut html, warning);
            html.push_str("</li>");
        }
        html.push_str("</ul>");
    }
    html.push_str("</section>");

    report_json_section(
        &mut html,
        "loadout-heading",
        "Loadout timeline and setup actions",
        root.get("loadoutTimeline").unwrap_or(&Value::Null),
    )?;
    report_json_section(
        &mut html,
        "approvals-heading",
        "User decisions and color mappings",
        root.get("userApprovals").unwrap_or(&Value::Null),
    )?;
    let empty_exclusions = Value::Array(Vec::new());
    report_json_section(
        &mut html,
        "excluded-heading",
        "Explicitly excluded source units",
        root.get("excludedSourceUnits").unwrap_or(&empty_exclusions),
    )?;
    report_json_section(&mut html, "manifest-heading", "Complete manifest", manifest)?;
    html.push_str("</main></body></html>\n");
    Ok(html.into_bytes())
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    entity: &str,
) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("A {entity} has no {key} value."))
}

fn report_field(html: &mut String, label: &str, value: &str) {
    html.push_str("<dt>");
    escape_html_into(html, label);
    html.push_str("</dt><dd><code>");
    escape_html_into(html, value);
    html.push_str("</code></dd>");
}

fn report_cell(html: &mut String, value: &str) {
    html.push_str("<td>");
    escape_html_into(html, value);
    html.push_str("</td>");
}

fn report_json_section(
    html: &mut String,
    id: &str,
    title: &str,
    value: &Value,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize {title}: {error}"))?;
    html.push_str("<section aria-labelledby=\"");
    escape_html_into(html, id);
    html.push_str("\"><h2 id=\"");
    escape_html_into(html, id);
    html.push_str("\">");
    escape_html_into(html, title);
    html.push_str("</h2><details><summary>Show details</summary><pre>");
    escape_html_into(html, &json);
    html.push_str("</pre></details></section>");
    Ok(())
}

fn escape_html_into(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Value {
        serde_json::json!({
            "schemaVersion": 2,
            "converter": {
                "name": "U1 3MF Color Planner",
                "buildIdentity": {
                    "packageVersion": "0.1.0",
                    "gitIdentity": "Unknown"
                }
            },
            "source": {
                "sha256": "a".repeat(64),
                "fileName": "fixture.3mf",
                "application": "bambu_studio",
                "applicationVersion": "01.10.02.00",
                "dialect": "bambu_studio_project"
            },
            "planFingerprint": "b".repeat(64),
            "artifacts": [{
                "fileName": "plate<&>.3mf",
                "printer": "U1",
                "strategy": "CMY+X Solid",
                "plateCount": 1,
                "loadout": [{ "toolhead": "T4", "spoolName": "Black <PLA>", "material": "PLA" }],
                "sha256": "c".repeat(64),
                "validationStatus": "Passed"
            }],
            "sourceToTargetMap": [{
                "sourceUnitId": "source-1",
                "outputFile": "plate<&>.3mf",
                "targetPlateId": "plate-1",
                "printer": "U1",
                "strategy": "CMY+X Solid"
            }],
            "userApprovals": [],
            "loadoutTimeline": [],
            "excludedSourceUnits": [{
                "scopeId": "scope-2",
                "planningUnitId": "unit-2",
                "sourcePlateId": 2,
                "sourceUnitId": "source-2",
                "reason": "No schedulable recipe",
                "errorIdentity": format!("sha256:{}", "d".repeat(64))
            }],
            "warningsAcknowledged": true,
            "acknowledgedWarnings": ["Review <toolhead> mapping"],
            "validationStatus": "Passed",
            "warnings": ["Review <toolhead> mapping"]
        })
    }

    #[test]
    fn report_is_deterministic_offline_and_escapes_untrusted_text() {
        let first = render_conversion_report(&manifest()).unwrap();
        let second = render_conversion_report(&manifest()).unwrap();
        assert_eq!(first, second);
        let html = String::from_utf8(first).unwrap();
        assert!(html.contains("Content-Security-Policy"));
        assert!(!html.contains("plate<&>.3mf"));
        assert!(html.contains("plate&lt;&amp;&gt;.3mf"));
        assert!(html.contains("Review &lt;toolhead&gt; mapping"));
        assert!(html.contains("Source to target mapping"));
        assert!(html.contains("User decisions and color mappings"));
        assert!(html.contains("Explicitly excluded source units</dt><dd>1"));
        assert!(html.contains("Warnings acknowledged</dt><dd>Yes"));
        assert!(html.contains("Source file</dt><dd><code>fixture.3mf"));
        assert!(html.contains("Converter git identity</dt><dd><code>Unknown"));
    }

    #[test]
    fn report_rejects_an_incomplete_manifest() {
        let error = render_conversion_report(&serde_json::json!({})).unwrap_err();
        assert!(error.contains("source identity"));
    }

    #[test]
    fn report_rejects_warning_acknowledgement_for_a_different_list() {
        let mut value = manifest();
        value["acknowledgedWarnings"] = serde_json::json!(["Different warning"]);

        let error = render_conversion_report(&value).unwrap_err();

        assert!(error.contains("does not match its exact warning list"));
    }
}
