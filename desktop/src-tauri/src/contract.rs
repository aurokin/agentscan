use crate::runner::{AgentscanRunner, classify_desktop_failure};

// Schema version of the `hotkeys --format json` envelope the host emits. The CLI
// wraps its picker rows in `{ "schema_version": 1, "rows": [...] }` so a row-shape
// change is a versioned break instead of a silent one; the desktop validates this
// before trusting the rows.
const PICKER_ROWS_SCHEMA_VERSION: u32 = 1;

// Versioned envelope emitted by `agentscan hotkeys --format json`. Rows travel
// under `rows`; `schema_version` gates compatibility.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(crate) struct PickerRowsEnvelope {
    pub(crate) schema_version: u32,
    pub(crate) rows: Vec<PickerRow>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PickerRow {
    pub(crate) key: String,
    pub(crate) pane_id: String,
    pub(crate) provider: Option<String>,
    pub(crate) status: PickerStatus,
    pub(crate) display_label: String,
    pub(crate) location_tag: String,
    pub(crate) location: PickerLocation,
    #[serde(flatten)]
    pub(crate) extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PickerStatus {
    pub(crate) kind: String,
    #[serde(flatten)]
    pub(crate) extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PickerLocation {
    pub(crate) session_name: String,
    #[serde(flatten)]
    pub(crate) extra: serde_json::Map<String, serde_json::Value>,
}

pub(crate) fn picker_rows_from_envelope(
    runner: &AgentscanRunner,
    envelope: PickerRowsEnvelope,
) -> Result<Vec<PickerRow>, String> {
    if envelope.schema_version != PICKER_ROWS_SCHEMA_VERSION {
        return Err(classify_desktop_failure(
            runner,
            "hotkeys",
            &format!(
                "Incompatible agentscan hotkeys schema_version {} (expected {PICKER_ROWS_SCHEMA_VERSION})",
                envelope.schema_version
            ),
        ));
    }
    Ok(envelope.rows)
}

pub(crate) fn validate_picker_rows(rows: &[PickerRow]) -> Result<(), String> {
    for row in rows {
        if row.key.trim().is_empty() {
            return Err("Incompatible agentscan hotkeys output: row key is empty".to_owned());
        }

        if row.pane_id.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty pane_id",
                row.key
            ));
        }

        if row.display_label.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty display_label",
                row.key
            ));
        }

        if row.location_tag.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty location_tag",
                row.key
            ));
        }

        if row
            .provider
            .as_deref()
            .is_some_and(|provider| provider.trim().is_empty())
        {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty provider",
                row.key
            ));
        }

        if row.status.kind.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty status kind",
                row.key
            ));
        }

        if row.location.session_name.trim().is_empty() {
            return Err(format!(
                "Incompatible agentscan hotkeys output: row {} has an empty session_name",
                row.key
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::LocalRunnerSettings;

    #[test]
    fn picker_rows_accept_empty_output() {
        let rows: Vec<PickerRow> = serde_json::from_str("[]").expect("empty rows parse");

        assert!(validate_picker_rows(&rows).is_ok());
    }

    #[test]
    fn picker_rows_envelope_unwraps_rows_at_supported_schema() {
        let envelope: PickerRowsEnvelope =
            serde_json::from_str(r#"{ "schema_version": 1, "rows": [] }"#)
                .expect("envelope parses");
        let runner = AgentscanRunner::Local(LocalRunnerSettings {
            binary_path: None,
            env: Vec::new(),
        });

        let rows = picker_rows_from_envelope(&runner, envelope).expect("supported schema unwraps");
        assert!(rows.is_empty());
    }

    #[test]
    fn picker_rows_envelope_rejects_unsupported_schema() {
        let envelope: PickerRowsEnvelope =
            serde_json::from_str(r#"{ "schema_version": 2, "rows": [] }"#)
                .expect("envelope parses");
        let runner = AgentscanRunner::Local(LocalRunnerSettings {
            binary_path: None,
            env: Vec::new(),
        });

        let error = picker_rows_from_envelope(&runner, envelope).unwrap_err();
        assert!(error.contains("schema_version"));
    }

    #[test]
    fn picker_rows_parse_contract_fields_and_preserve_extra_fields() {
        let rows: Vec<PickerRow> = serde_json::from_str(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": "codex",
                "status": { "kind": "idle" },
	                "display_label": "Root Task",
	                "location_tag": "work:0.0",
	                "location": { "session_name": "work" },
	                "workspace": { "label": "agentscan", "source": "git_repo" },
	                "display": { "provider_marker": "💭" }
	              }
	            ]"#,
        )
        .expect("picker row parses");

        assert!(validate_picker_rows(&rows).is_ok());
        assert_eq!(rows[0].key, "1");
        assert_eq!(rows[0].pane_id, "%1");
        assert_eq!(rows[0].provider.as_deref(), Some("codex"));
        assert_eq!(rows[0].status.kind, "idle");
        assert_eq!(rows[0].location.session_name, "work");
        assert_eq!(
            rows[0].extra["workspace"]["label"].as_str(),
            Some("agentscan")
        );
        assert!(rows[0].extra.contains_key("display"));
    }

    #[test]
    fn picker_row_waiting_status_round_trips() {
        let input = serde_json::json!({
            "key": "1",
            "pane_id": "%1",
            "provider": "codex",
            "status": { "kind": "waiting" },
            "display_label": "Root Task",
            "location_tag": "work:0.0",
            "location": { "session_name": "work" }
        });
        let row: PickerRow = serde_json::from_value(input.clone()).expect("picker row parses");

        assert_eq!(row.status.kind, "waiting");
        assert_eq!(
            serde_json::to_value(row).expect("picker row serializes"),
            input
        );
    }

    #[test]
    fn picker_rows_reject_incompatible_output() {
        let rows: Vec<PickerRow> = serde_json::from_str(
            r#"[
              {
                "key": "1",
                "pane_id": "",
                "provider": "codex",
                "status": { "kind": "idle" },
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .expect("picker row parses");

        assert!(validate_picker_rows(&rows).unwrap_err().contains("pane_id"));
    }

    #[test]
    fn picker_rows_reject_wrong_field_shapes() {
        let error = serde_json::from_str::<Vec<PickerRow>>(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": {},
                "status": { "kind": "idle" },
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("invalid type"));
    }

    #[test]
    fn picker_rows_reject_empty_nested_fields() {
        let rows: Vec<PickerRow> = serde_json::from_str(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": "codex",
                "status": { "kind": "" },
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .expect("picker row parses");

        assert!(
            validate_picker_rows(&rows)
                .unwrap_err()
                .contains("status kind")
        );
    }

    #[test]
    fn picker_rows_reject_missing_status() {
        let error = serde_json::from_str::<Vec<PickerRow>>(
            r#"[
              {
                "key": "1",
                "pane_id": "%1",
                "provider": "codex",
                "display_label": "Root Task",
                "location_tag": "work:0.0",
                "location": { "session_name": "work" }
              }
            ]"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("missing field `status`"));
    }
}
