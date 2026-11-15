use super::diagnostics::App;
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

const SOURCE: &str = r#"// Café 🧪: keep this source exactly as authored.
use Detail from "./detail.contract"

fn label(n: number): string = `Počet ${n}`

component App
  state busy = false
  state count = 1
  action toggle
    busy = not busy
  view
    column gap=8 min-width=0
      button "Go" press=toggle disabled=busy // same Café 🧪
      button "Go" press=toggle disabled=busy // same Café 🧪
      text `Café 😀 ${label(count)}`
        aria-label=(
          label(count)
        )
        testId="label"
      Detail(value=count)
"#;

const BUTTONS: &str = r#"component App
  state busy = false
  state ready = true
  action flip
    busy = not busy
  view
    column
      button press=flip disabled=busy testId="launch" // Café 🧪
        text "Launch"
      button press=flip disabled=(not ready) testId="archive"
        text "Archive"
      text "Launch"
"#;

const SELECTORS: &str = r#"component App
  state busy = false
  action act
    busy = not busy
  view
    column
      button press=act disabled=busy aria-label="Run\u0020now" retainFocus=true testId="chosen" // Café
        text "Run"
      button press=act disabled=busy aria-label="Run now" testId="absent"
        text "Run"
      button press=act disabled=busy aria-label="Run  now" retainFocus=true testId="spaces"
        text "Run"
      button press=act disabled=(busy) aria-label="Run now" retainFocus=true testId="parenthesized"
        text "Run"
      input value="" autofocus testId="entry"
      h1 "Run heading"
component Other
  props
    busy: bool
  view
    button disabled=busy aria-label="Run now" retainFocus=true testId="other"
      text "Run"
"#;

fn fixture(name: &str) -> App {
    let app = App::new(name);
    let root = app.write("app.contract", SOURCE);
    app.write(
        "detail.contract",
        "component Detail\n  props\n    value: number\n  view\n    text value.toString()\n",
    );
    contract::compile_path(&root).unwrap();
    app
}

fn response(output: Output, code: i32) -> Value {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).expect("one JSON response")
}

fn inspect(app: &App, args: &[&str]) -> Value {
    response(
        Command::new(env!("CARGO_BIN_EXE_contract"))
            .args(["inspect", "app.contract", "--json"])
            .args(args)
            .current_dir(&app.0)
            .output()
            .unwrap(),
        0,
    )
}

fn edit(app: &App, request: Value, apply: bool, code: i32) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_contract"));
    command
        .args(["edit", "app.contract", "--stdin", "--json"])
        .current_dir(&app.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if apply {
        command.arg("--apply");
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&request).unwrap().as_bytes())
        .unwrap();
    response(child.wait_with_output().unwrap(), code)
}

fn nodes<'a>(body: &'a Value, tag: &str) -> Vec<&'a Value> {
    let mut nodes: Vec<_> = body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["tag"] == tag)
        .collect();
    nodes.sort_by_key(|node| node["range"]["start"].as_u64().unwrap());
    nodes
}

fn attr<'a>(node: &'a Value, name: &str) -> &'a Value {
    node["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|attr| attr["name"] == name)
        .unwrap_or_else(|| panic!("missing {name}: {node}"))
}

fn source(app: &App) -> String {
    std::fs::read_to_string(app.0.join("app.contract")).unwrap()
}

fn cli(app: &App, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_contract"))
        .args(args)
        .current_dir(&app.0)
        .output()
        .unwrap()
}

#[test]
fn inspection_exposes_selected_source_nodes_without_expanding_imports() {
    let app = fixture("structural-inspect");
    let index = inspect(&app, &[]);
    assert_eq!(index["version"], 1);
    assert!(!index["revision"].as_str().unwrap().is_empty());
    assert!(index["nodes"].as_array().unwrap().is_empty());
    assert_eq!(index["components"].as_array().unwrap().len(), 1);
    assert_eq!(index["components"][0]["name"], "App");

    let body = inspect(&app, &["--component", "App"]);
    assert_eq!(body["revision"], index["revision"]);
    let authored = body["nodes"].as_array().unwrap();
    let handles: std::collections::BTreeSet<_> = authored
        .iter()
        .map(|node| node["handle"].clone().to_string())
        .collect();
    assert_eq!(handles.len(), authored.len());
    assert!(authored.iter().all(|node| node["parent"] != node["handle"]));
    assert_eq!(nodes(&body, "text").len(), 1);
    let buttons = nodes(&body, "button");
    assert_eq!(buttons.len(), 2);
    assert_ne!(buttons[0]["handle"], buttons[1]["handle"]);
    assert_ne!(buttons[0]["range"], buttons[1]["range"]);
    assert_eq!(buttons[0]["component"], "App");
    for button in buttons {
        let start = button["range"]["start"].as_u64().unwrap() as usize;
        let end = button["range"]["end"].as_u64().unwrap() as usize;
        assert!(SOURCE.is_char_boundary(start) && SOURCE.is_char_boundary(end));
        assert!(SOURCE[start..end].contains("button \"Go\""));
        assert_eq!(attr(button, "disabled")["value"], "busy");
    }
    let matches = inspect(&app, &["--component", "App", "--match", "button"]);
    assert_eq!(nodes(&matches, "button").len(), 2);
    assert_eq!(source(&app), SOURCE);
}

#[test]
fn preview_and_apply_change_only_the_selected_duplicate_attribute() {
    let app = fixture("structural-preview");
    let body = inspect(&app, &["--component", "App"]);
    let target = attr(nodes(&body, "button")[1], "disabled");
    let request = json!({
        "revision": body["revision"],
        "edits": [{"op": "replace-expr", "target": target["handle"], "value": "false"}]
    });
    let preview = edit(&app, request.clone(), false, 0);
    assert_eq!(preview["ok"], true);
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["revision"], body["revision"]);
    assert_ne!(preview["newRevision"], body["revision"]);
    assert_eq!(source(&app), SOURCE);

    let applied = edit(&app, request, true, 0);
    assert_eq!(applied["ok"], true);
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["newRevision"], preview["newRevision"]);
    let at = SOURCE.rfind("disabled=busy").unwrap() + "disabled=".len();
    let expected = format!("{}false{}", &SOURCE[..at], &SOURCE[at + "busy".len()..]);
    assert_eq!(source(&app), expected);
    contract::compile_path(&app.0.join("app.contract")).unwrap();
    assert_eq!(inspect(&app, &[])["revision"], applied["newRevision"]);
}

#[test]
fn a_valid_batch_preserves_templates_comments_and_continued_attributes() {
    let app = fixture("structural-batch");
    let body = inspect(&app, &["--component", "App"]);
    let column = nodes(&body, "column")[0];
    let button = nodes(&body, "button")[1];
    let text = nodes(&body, "text")[0];
    let changed = edit(
        &app,
        json!({"revision": body["revision"], "edits": [
            {"op": "remove-attr", "target": column["handle"], "name": "min-width"},
            {"op": "set-attr", "target": button["handle"], "name": "testId", "value": "\"second\""},
            {"op": "replace-expr", "target": attr(text, "aria-label")["handle"], "value": "label(count + 1)"}
        ]}),
        true,
        0,
    );
    assert_eq!(changed["ok"], true);
    assert_eq!(changed["applied"], true);
    assert_eq!(changed["changes"].as_array().unwrap().len(), 3);
    let expected = SOURCE
        .replace("column gap=8 min-width=0", "column gap=8")
        .replace(
            "aria-label=(\n          label(count)\n        )",
            "aria-label=label(count + 1)",
        );
    let header = "button \"Go\" press=toggle disabled=busy";
    let end = expected.rfind(header).unwrap() + header.len();
    let expected = format!("{} testId=\"second\"{}", &expected[..end], &expected[end..]);
    assert_eq!(source(&app), expected);
    contract::compile_path(&app.0.join("app.contract")).unwrap();
    assert_eq!(
        std::fs::read_to_string(app.0.join("detail.contract")).unwrap(),
        "component Detail\n  props\n    value: number\n  view\n    text value.toString()\n"
    );
}

#[test]
fn stale_revisions_and_overlapping_edits_leave_authored_source_unchanged() {
    let app = fixture("structural-conflict");
    let body = inspect(&app, &["--component", "App"]);
    let target = attr(nodes(&body, "button")[0], "disabled");
    let changed = format!("{SOURCE}// a later local edit\n");
    app.write("app.contract", &changed);
    let stale = edit(
        &app,
        json!({"revision": body["revision"], "edits": [
            {"op": "replace-expr", "target": target["handle"], "value": "false"}
        ]}),
        true,
        1,
    );
    assert_eq!(stale["ok"], false);
    assert_eq!(stale["applied"], false);
    assert!(stale["error"]["id"].is_string());
    assert_eq!(source(&app), changed);

    let current = inspect(&app, &["--component", "App"]);
    let target = attr(nodes(&current, "button")[0], "disabled");
    let conflict = edit(
        &app,
        json!({"revision": current["revision"], "edits": [
            {"op": "replace-expr", "target": target["handle"], "value": "true"},
            {"op": "replace-expr", "target": target["handle"], "value": "false"}
        ]}),
        true,
        1,
    );
    assert_eq!(conflict["ok"], false);
    assert_eq!(conflict["applied"], false);
    assert_eq!(source(&app), changed);
}

#[test]
fn candidate_type_errors_are_returned_before_any_source_write() {
    let app = fixture("structural-invalid-type");
    let body = inspect(&app, &["--component", "App"]);
    let button = nodes(&body, "button")[0];
    let refused = edit(
        &app,
        json!({"revision": body["revision"], "edits": [
            {"op": "set-attr", "target": button["handle"], "name": "disabled", "value": "1"}
        ]}),
        true,
        1,
    );
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["applied"], false);
    assert!(!refused["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(source(&app), SOURCE);
    assert_eq!(inspect(&app, &[])["revision"], body["revision"]);
}

#[test]
fn expression_edits_cannot_add_attributes_or_view_nodes() {
    let app = fixture("structural-expression-boundary");
    let body = inspect(&app, &["--component", "App"]);
    let button = nodes(&body, "button")[0];
    for operation in [
        json!({"op": "set-attr", "target": button["handle"], "name": "disabled", "value": "false testId=\"injected\""}),
        json!({"op": "replace-expr", "target": attr(button, "disabled")["handle"], "value": "false testId=\"injected\""}),
        json!({"op": "replace-expr", "target": attr(button, "disabled")["handle"], "value": "false\n      text \"Injected\""}),
    ] {
        let refused = edit(
            &app,
            json!({"revision": body["revision"], "edits": [operation]}),
            true,
            1,
        );
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["applied"], false);
        assert_eq!(source(&app), SOURCE);
    }
}

#[test]
fn validation_checks_a_component_argument_against_its_imported_definition() {
    let app = fixture("structural-imported-argument");
    let body = inspect(&app, &["--component", "App"]);
    let call = nodes(&body, "Detail")[0];
    let refused = edit(
        &app,
        json!({"revision": body["revision"], "edits": [
            {"op": "replace-expr", "target": attr(call, "value")["handle"], "value": "\"wrong type\""}
        ]}),
        true,
        1,
    );
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["applied"], false);
    assert!(!refused["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(source(&app), SOURCE);
}

#[test]
fn direct_match_groups_apply_and_validate_one_source_preserving_batch() {
    let app = fixture("structural-direct-batch");
    let args = [
        "edit",
        "app.contract",
        "--component",
        "App",
        "--match",
        "column gap=8",
        "--set",
        "gap=12",
        "--unset",
        "min-width",
        "--match",
        "text `Café",
        "--set",
        "aria-label=label(count + 1)",
    ];
    let preview = response(cli(&app, &args), 0);
    assert_eq!(preview["ok"], true);
    assert_eq!(preview["applied"], false);
    assert_eq!(preview["validated"], true);
    assert!(preview["diff"].as_str().unwrap().contains("gap=12"));
    assert_eq!(source(&app), SOURCE);

    let mut apply_args = args.to_vec();
    apply_args.push("--apply");
    let applied = response(cli(&app, &apply_args), 0);
    assert_eq!(applied["ok"], true);
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["validated"], true);
    assert_eq!(applied["diff"], preview["diff"]);
    let expected = SOURCE
        .replace("column gap=8 min-width=0", "column gap=12")
        .replace(
            "aria-label=(\n          label(count)\n        )",
            "aria-label=label(count + 1)",
        );
    assert_eq!(source(&app), expected);
    contract::compile_path(&app.0.join("app.contract")).unwrap();
}

#[test]
fn direct_matches_refuse_missing_ambiguous_and_invalid_batches_without_writing() {
    let app = fixture("structural-direct-refusals");
    let cases: &[&[&str]] = &[
        &["--match", "not-a-source-node", "--set", "disabled=true"],
        &["--match", "button \"Go\"", "--set", "disabled=true"],
        &["--match", "column gap=8", "--set", "gap=\"wrong type\""],
        &[
            "--match",
            "column gap=8",
            "--set",
            "gap=1 testId=\"injected\"",
        ],
        &[
            "--match",
            "column gap=8",
            "--set",
            "gap=12",
            "--match",
            "not-a-source-node",
            "--set",
            "disabled=true",
        ],
    ];
    for case in cases {
        let mut args = vec!["edit", "app.contract", "--apply"];
        args.extend_from_slice(case);
        let refused_output = cli(&app, &args);
        assert!(refused_output.stdout.len() < 8000);
        let refused = response(refused_output, 1);
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["applied"], false);
        assert_eq!(source(&app), SOURCE);
    }
}

#[test]
fn inspection_batches_deduplicate_order_and_cap_a_single_union() {
    let app = App::new("structural-compact-inspect");
    let rows = (0..45)
        .map(|n| format!("      text \"row {n}\" testId=\"row-{n}\"\n"))
        .collect::<String>();
    let original = format!("component App\n  view\n    column\n{rows}");
    let root = app.write("app.contract", &original);
    contract::compile_path(&root).unwrap();
    let args = [
        "inspect",
        "app.contract",
        "--component",
        "App",
        "--tag",
        "text",
        "--match",
        "row 2",
        "--match",
        "row",
        "--match",
        "row 2",
    ];
    let compact_output = cli(&app, &args);
    let compact_bytes = compact_output.stdout.len();
    let compact = response(compact_output, 0);
    let rows = compact["nodes"].as_array().unwrap();
    assert_eq!(rows.len(), 20);
    assert_eq!(compact["total"], 45);
    assert_eq!(compact["more"], 25);
    for (n, row) in rows.iter().enumerate() {
        assert_eq!(row["component"], "App");
        assert!(row["line"].as_u64().unwrap() > 0);
        assert!(row["handle"].is_string());
        assert_eq!(
            row["source"],
            format!("text \"row {n}\" testId=\"row-{n}\"")
        );
        assert!(row.get("attributes").is_none());
    }
    let mut detailed_args = args.to_vec();
    detailed_args.push("--json");
    let detailed_output = cli(&app, &detailed_args);
    assert!(detailed_output.stdout.len() > compact_bytes * 2);
    let detailed = response(detailed_output, 0);
    assert_eq!(nodes(&detailed, "text").len(), 45);
    let selected = detailed["nodes"].as_array().unwrap();
    assert_eq!(selected.len(), 45);
    for (n, row) in selected.iter().enumerate() {
        assert_eq!(attr(row, "testId")["value"], format!("\"row-{n}\""));
    }
    let none = response(
        cli(
            &app,
            &[
                "inspect",
                "app.contract",
                "--match",
                "row 90",
                "--match",
                "absent",
            ],
        ),
        0,
    );
    assert!(none["nodes"].as_array().unwrap().is_empty());
    assert_eq!(none["total"], 0);
    assert_eq!(none["more"], 0);
    assert_eq!(source(&app), original);
}

#[test]
fn successful_edits_summarize_many_warnings_and_offer_full_diagnostics() {
    let app = App::new("structural-compact-diagnostics");
    let rows = (0..40)
        .map(|n| format!("      text \"row {n}\" color=\"#ff0000\" testId=\"row-{n}\"\n"))
        .collect::<String>();
    let original = format!("component App\n  view\n    column\n{rows}");
    let root = app.write("app.contract", &original);
    let (checked, warnings) = contract::compile_path_all_with_warnings(&root, false);
    checked.unwrap();
    assert!(warnings.len() >= 20);
    let args = [
        "edit",
        "app.contract",
        "--match",
        "testId=\"row-0\"",
        "--set",
        "aria-label=\"Changed row\"",
    ];
    let compact_output = cli(&app, &args);
    let compact_bytes = compact_output.stdout.len();
    assert!(compact_bytes < 8000);
    let compact = response(compact_output, 0);
    assert_eq!(compact["ok"], true);
    assert_eq!(compact["validated"], true);
    assert_eq!(compact["applied"], false);
    assert!(compact["warnings"]["count"].as_u64().unwrap() >= 20);
    assert!(compact["diff"].as_str().unwrap().contains("Changed row"));
    let mut detailed_args = args.to_vec();
    detailed_args.extend(["--json", "--diagnostics"]);
    let detailed_output = cli(&app, &detailed_args);
    assert!(detailed_output.stdout.len() > compact_bytes * 2);
    let detailed = response(detailed_output, 0);
    assert!(detailed["diagnostics"].as_array().unwrap().len() >= 20);
    assert_eq!(
        compact["warnings"]["count"].as_u64().unwrap() as usize,
        detailed["diagnostics"].as_array().unwrap().len()
    );
    assert!(detailed["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .all(|diagnostic| diagnostic["severity"] == "warning"));
    assert_eq!(source(&app), original);
}

#[test]
fn direct_edit_keeps_the_builds_ambient_api_refusals_before_applying() {
    let app = App::new("structural-ambient-parity");
    let original = "component App\n  view\n    text \"Hello\" testId=\"greeting\"\n";
    app.write("app.contract", original);
    app.write("app.ts", "export const answer = () => Date.now();\n");
    let bun = std::env::var_os("BUN").unwrap_or_else(|| "bun".into());
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_contract"))
            .args(args)
            .current_dir(&app.0)
            .env("BUN", &bun)
            .output()
            .unwrap()
    };
    let build = response(
        run(&["build", "app.contract", "--json", "--errors-only"]),
        1,
    );
    let build_errors = build.as_array().unwrap();
    assert!(build_errors
        .iter()
        .any(|error| error["id"] == "analyze-ambient-api"
            && error["message"].as_str().unwrap().contains("Date.now()")));

    let args = [
        "edit",
        "app.contract",
        "--match",
        "testId=\"greeting\"",
        "--set",
        "aria-label=\"Hello\"",
        "--apply",
    ];
    let edit = response(run(&args), 1);
    assert_eq!(edit["ok"], false);
    assert_eq!(edit["applied"], false);
    let error_ids = |errors: &[Value]| {
        errors
            .iter()
            .filter(|error| error["severity"] == "error")
            .map(|error| error["id"].as_str().unwrap().to_owned())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        error_ids(edit["diagnostics"].as_array().unwrap()),
        error_ids(build_errors)
    );
    assert_eq!(source(&app), original);

    app.write("app.ts", "export const answer = () => 7;\n");
    let accepted = response(run(&args), 0);
    assert_eq!(accepted["applied"], true);
    assert_eq!(accepted["validated"], true);
    assert_eq!(
        source(&app),
        original.replace(
            "testId=\"greeting\"",
            "testId=\"greeting\" aria-label=\"Hello\""
        )
    );
}

#[test]
fn tagged_matches_select_buttons_by_child_text_and_refuse_ambiguous_owners() {
    let app = App::new("structural-tagged-button");
    app.write("app.contract", BUTTONS);
    let own_headers = inspect(&app, &["--match", "Launch"]);
    assert_eq!(nodes(&own_headers, "button").len(), 0);
    assert_eq!(nodes(&own_headers, "text").len(), 2);
    let owners = inspect(&app, &["--tag", "button", "--match", "Launch"]);
    assert_eq!(nodes(&owners, "button").len(), 1);
    assert_eq!(owners["nodes"].as_array().unwrap().len(), 1);
    let applied = response(
        cli(
            &app,
            &[
                "edit",
                "app.contract",
                "--tag",
                "button",
                "--match",
                "Launch",
                "--set",
                "aria-label=\"Launch job\"",
                "--apply",
            ],
        ),
        0,
    );
    assert_eq!(applied["validated"], true);
    assert_eq!(
        source(&app),
        BUTTONS.replace(
            "testId=\"launch\"",
            "testId=\"launch\" aria-label=\"Launch job\""
        )
    );
    let multiple_components = format!(
        "{BUTTONS}component Other\n  view\n    button testId=\"other\"\n      text \"Launch\"\n"
    );
    app.write("app.contract", &multiple_components);
    let union = inspect(
        &app,
        &[
            "--component",
            "App",
            "--tag",
            "button",
            "--match",
            "Archive",
            "--match",
            "Launch",
            "--match",
            "Launch",
        ],
    );
    let owners = union["nodes"].as_array().unwrap();
    assert_eq!(owners.len(), 2);
    assert_eq!(attr(&owners[0], "testId")["value"], "\"launch\"");
    assert_eq!(attr(&owners[1], "testId")["value"], "\"archive\"");
    assert!(owners
        .iter()
        .all(|node| node["component"] == "App" && node["tag"] == "button"));
    assert_eq!(source(&app), multiple_components);
    let ambiguous = BUTTONS.replace("text \"Archive\"", "text \"Launch\"");
    app.write("app.contract", &ambiguous);
    let refused = response(
        cli(
            &app,
            &[
                "edit",
                "app.contract",
                "--tag",
                "button",
                "--match",
                "Launch",
                "--set",
                "disabled=true",
                "--apply",
            ],
        ),
        1,
    );
    assert_eq!(refused["applied"], false);
    assert_eq!(source(&app), ambiguous);
}

#[test]
fn direct_boolean_composition_keeps_existing_expression_and_consumer_behavior() {
    let app = App::new("structural-boolean-composition");
    app.write("app.contract", BUTTONS);
    let disabled = |text: &str| {
        let runner = exact_runner::Runner::boot(
            contract::compile(text).unwrap(),
            (),
            exact_kernel::Kernel::with_monospace(),
            Default::default(),
            "/",
        )
        .unwrap();
        let key = runner.kernel().find_by_test_id("launch")[0];
        runner
            .kernel()
            .node_by_key(key)
            .unwrap()
            .props
            .bool(exact_kernel::PropId::Disabled)
    };
    let applied = response(
        cli(
            &app,
            &[
                "edit",
                "app.contract",
                "--tag",
                "button",
                "--match",
                "Launch",
                "--or",
                "disabled=ready",
                "--apply",
            ],
        ),
        0,
    );
    assert_eq!(applied["validated"], true);
    let after_or = BUTTONS.replace("disabled=busy", "disabled=(busy) or (ready)");
    assert_eq!(source(&app), after_or);
    assert_eq!(disabled(&source(&app)), Some(true));
    response(
        cli(
            &app,
            &[
                "edit",
                "app.contract",
                "--tag",
                "button",
                "--match",
                "Launch",
                "--and",
                "disabled=not ready",
                "--apply",
            ],
        ),
        0,
    );
    assert_eq!(
        source(&app),
        after_or.replace(
            "disabled=(busy) or (ready)",
            "disabled=((busy) or (ready)) and (not ready)"
        )
    );
    assert_eq!(disabled(&source(&app)), Some(false));
}

#[test]
fn json_boolean_composition_retains_snapshot_and_transactional_refusal_guards() {
    let app = fixture("structural-json-boolean");
    let body = inspect(&app, &["--component", "App"]);
    let buttons = nodes(&body, "button");
    let applied = edit(
        &app,
        json!({"revision": body["revision"], "edits": [
            {"op": "or-attr", "target": buttons[0]["handle"], "name": "disabled", "value": "true"},
            {"op": "and-attr", "target": buttons[1]["handle"], "name": "disabled", "value": "false"}
        ]}),
        true,
        0,
    );
    assert_eq!(applied["validated"], true);
    let expected = SOURCE.replacen("disabled=busy", "disabled=(busy) or (true)", 1);
    let expected = expected.replace("disabled=busy", "disabled=(busy) and (false)");
    assert_eq!(source(&app), expected);
    let stale = edit(
        &app,
        json!({"revision": body["revision"], "edits": [
            {"op": "or-attr", "target": buttons[0]["handle"], "name": "disabled", "value": "false"}
        ]}),
        true,
        1,
    );
    assert_eq!(stale["applied"], false);
    assert_eq!(source(&app), expected);

    let current = inspect(&app, &["--component", "App"]);
    let target = &nodes(&current, "button")[0]["handle"];
    for operations in [
        json!([
            {"op": "or-attr", "target": target, "name": "disabled", "value": "true"},
            {"op": "and-attr", "target": target, "name": "disabled", "value": "false"}
        ]),
        json!([{"op": "or-attr", "target": target, "name": "disabled", "value": "true testId=\"injected\""}]),
        json!([{"op": "and-attr", "target": target, "name": "disabled", "value": "42"}]),
        json!([{"op": "or-attr", "target": target, "name": "aria-hidden", "value": "true"}]),
    ] {
        let refused = edit(
            &app,
            json!({"revision": current["revision"], "edits": operations}),
            true,
            1,
        );
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["applied"], false);
        assert_eq!(source(&app), expected);
    }
}

#[test]
fn compound_selectors_disambiguate_tagged_owners_before_preserving_source_edits() {
    let app = App::new("structural-compound-selectors");
    contract::compile(SELECTORS).unwrap();
    app.write("app.contract", SELECTORS);
    let selectors = [
        "--component",
        "App",
        "--tag",
        "button",
        "--match",
        "Run",
        "--has",
        "retainFocus",
        "--has",
        "disabled",
        "--where",
        "aria-label = \"Run now\"",
        "--where",
        "disabled = busy",
    ];
    let selected = inspect(&app, &selectors);
    assert_eq!(selected["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(attr(&selected["nodes"][0], "testId")["value"], "\"chosen\"");
    let mut args = vec!["edit", "app.contract"];
    args.extend(selectors);
    args.extend(["--set", "aria-label=\"Selected\"", "--apply"]);
    let applied = response(cli(&app, &args), 0);
    assert_eq!(applied["validated"], true);
    assert_eq!(
        source(&app),
        SELECTORS.replacen("aria-label=\"Run\\u0020now\"", "aria-label=\"Selected\"", 1)
    );
}

#[test]
fn lexical_constraints_keep_string_contents_and_parentheses_without_evaluation() {
    let app = App::new("structural-lexical-selectors");
    contract::compile(SELECTORS).unwrap();
    app.write("app.contract", SELECTORS);
    for (condition, expected) in [
        ("disabled=busy", vec!["chosen", "absent", "spaces"]),
        ("disabled=( busy )", vec!["parenthesized"]),
        ("disabled=false", vec![]),
        ("aria-label=\"Run  now\"", vec!["spaces"]),
        ("aria-label=\"Run\"", vec![]),
    ] {
        let selected = inspect(
            &app,
            &[
                "--component",
                "App",
                "--tag",
                "button",
                "--where",
                condition,
            ],
        );
        let selected = selected["nodes"].as_array().unwrap();
        let ids: Vec<_> = selected
            .iter()
            .map(|node| attr(node, "testId")["value"].as_str().unwrap().to_owned())
            .collect();
        let expected: Vec<_> = expected.into_iter().map(|id| format!("\"{id}\"")).collect();
        assert_eq!(ids, expected, "{condition}");
    }
    let bare = inspect(&app, &["--has", "autofocus"]);
    assert_eq!(bare["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(attr(&bare["nodes"][0], "testId")["value"], "\"entry\"");
    assert!(inspect(&app, &["--has", "role"])["nodes"]
        .as_array()
        .unwrap()
        .is_empty());
    for constraints in [
        vec!["--where", "disabled=busy", "--where", "disabled=(busy)"],
        vec!["--has", "nonexistent"],
        vec!["--where", "disabled=busy trailing"],
    ] {
        let mut args = vec![
            "edit",
            "app.contract",
            "--component",
            "App",
            "--tag",
            "button",
            "--match",
            "Run",
        ];
        args.extend(constraints);
        args.extend(["--set", "disabled=true", "--apply"]);
        let refused = response(cli(&app, &args), 1);
        assert_eq!(refused["applied"], false);
        assert!(refused["error"]["candidates"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(source(&app), SELECTORS);
    }
}

#[test]
fn optional_context_is_scoped_declared_bounded_and_empty_without_matches() {
    let app = App::new("structural-context");
    let original = r#"shape Reply
  ok: bool
component App
  state busy = false
  state count: number = 1
  state secret = "not-context-data"
  derive label = busy ? "Busy" : "Ready"
  resource status = loadStatus() as shape Reply else empty(ok=false)
  mutation saved as shape Reply
  provide
    theme = "light"
  action save(next: bool)
    busy = next
  view
    column
      button press=save(true) testId="root"
        text label
      Child(title="Child", act=save(true))
component Child
  props
    title: string
    act: action
  inject
    theme: string
  state local = title
  derive shown = `${local}:${theme}`
  view
    button press=act testId="child"
      text shown
"#;
    app.write("app.contract", original);
    let plain = inspect(&app, &["--where", "testId=\"root\""]);
    assert!(plain.get("context").is_none());
    let root = inspect(&app, &["--where", "testId=\"root\"", "--context"]);
    let scopes = root["context"].as_array().unwrap();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0]["component"], "App");
    let members = scopes[0]["members"].as_array().unwrap();
    for (name, kind, ty) in [
        ("status", "resource", Some("Reply")),
        ("saved", "mutation", Some("option<Reply>")),
        ("save", "action", Some("action")),
        ("count", "state", Some("number")),
        ("busy", "state", None),
        ("label", "derive", None),
    ] {
        let member = members.iter().find(|m| m["name"] == name).unwrap();
        assert_eq!(member["kind"], kind);
        assert_eq!(member["type"].as_str(), ty);
        assert!(member["line"].as_u64().unwrap() > 0);
    }
    let save = members.iter().find(|m| m["name"] == "save").unwrap();
    assert_eq!(save["params"][0]["name"], "next");
    assert_eq!(save["params"][0]["type"], "bool");
    assert!(!root["context"].to_string().contains("not-context-data"));

    let child = inspect(&app, &["--where", "testId=\"child\"", "--context"]);
    let scopes = child["context"].as_array().unwrap();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0]["component"], "Child");
    let members = scopes[0]["members"].as_array().unwrap();
    for (name, kind, ty) in [
        ("title", "prop", "string"),
        ("act", "prop", "action"),
        ("theme", "inject", "string"),
    ] {
        let member = members.iter().find(|m| m["name"] == name).unwrap();
        assert_eq!(member["kind"], kind);
        assert_eq!(member["type"], ty);
    }
    assert!(members.iter().all(|m| m["name"] != "status"));

    let states = (0..40)
        .map(|n| format!("  state extra{n} = {n}\n"))
        .collect::<String>();
    let mut large = original.replace(
        "  state busy = false\n",
        &format!("  state busy = false\n{states}"),
    );
    for n in 0..5 {
        large.push_str(&format!(
            "component Extra{n}\n  view\n    button testId=\"extra-{n}\"\n      text \"Extra\"\n"
        ));
    }
    app.write("app.contract", &large);
    let bounded = inspect(&app, &["--tag", "button", "--context"]);
    let contexts = bounded["context"].as_array().unwrap();
    assert_eq!(contexts.len(), 4);
    assert_eq!(bounded["contextMore"], 3);
    let root = contexts.iter().find(|c| c["component"] == "App").unwrap();
    let members = root["members"].as_array().unwrap();
    assert_eq!(members.len(), 32);
    assert_eq!(
        root["total"].as_u64().unwrap(),
        32 + root["more"].as_u64().unwrap()
    );
    assert!(root["omittedByKind"]["state"].as_u64().unwrap() > 0);
    for name in ["status", "saved", "save"] {
        assert!(members.iter().any(|m| m["name"] == name));
    }
    let empty = inspect(&app, &["--where", "testId=\"absent\"", "--context"]);
    assert!(empty["nodes"].as_array().unwrap().is_empty());
    assert!(empty["context"].as_array().unwrap().is_empty());
    assert_eq!(empty["contextMore"], 0);
    assert!(empty.get("suggestions").is_none());
    assert_eq!(source(&app), large);
}

#[test]
fn direct_text_edits_quote_plain_content_and_preserve_unrelated_source_in_a_batch() {
    let app = App::new("structural-text-batch");
    let original = r#"// Café 🧪 keeps its authored bytes.
component App
  state busy = false
  action flip
    busy = not busy
  view
    column gap=8
      button "Open \"draft\"" press=flip disabled=busy testId="project" // keep me
      text ("Delivery") testId="message"
        aria-label="Unchanged"
      button press=flip testId="nested"
        text "Start" testId="nested-label" // keep this too
"#;
    contract::compile(original).unwrap();
    app.write("app.contract", original);
    let content = "Příliš 🧪\n\"quoted\" \\ path\n      text \"Not a node\"";
    let applied = response(
        cli(
            &app,
            &[
                "edit",
                "app.contract",
                "--match",
                "testId=\"project\"",
                "--text",
                "Choose project",
                "--set",
                "disabled=true",
                "--match",
                "testId=\"message\"",
                "--text",
                content,
                "--match",
                "testId=\"nested\"",
                "--text",
                "Continue",
                "--apply",
            ],
        ),
        0,
    );
    assert_eq!(applied["validated"], true);
    assert_eq!(applied["changes"], 4);
    let expected = original
        .replace(r#""Open \"draft\"""#, r#""Choose project""#)
        .replace("disabled=busy", "disabled=true")
        .replace(
            r#"("Delivery")"#,
            r#"("Příliš 🧪\n\"quoted\" \\ path\n      text \"Not a node\"")"#,
        )
        .replace(r#"text "Start""#, r#"text "Continue""#);
    assert_eq!(source(&app), expected);

    let runner = exact_runner::Runner::boot(
        contract::compile(&source(&app)).unwrap(),
        (),
        exact_kernel::Kernel::with_monospace(),
        Default::default(),
        "/",
    )
    .unwrap();
    let kernel = runner.kernel();
    for (id, text) in [("message", content), ("nested-label", "Continue")] {
        let node = kernel.node_by_key(kernel.find_by_test_id(id)[0]).unwrap();
        assert_eq!(node.props.str(exact_kernel::PropId::Text), Some(text));
    }
    let project = kernel
        .node_by_key(kernel.find_by_test_id("project")[0])
        .unwrap();
    assert_eq!(
        project.props.bool(exact_kernel::PropId::Disabled),
        Some(true)
    );
}

#[test]
fn json_text_edits_keep_preview_snapshot_and_shared_literal_conflict_guards() {
    let app = App::new("structural-text-snapshot");
    app.write("app.contract", BUTTONS);
    let body = inspect(&app, &["--component", "App"]);
    let launch = nodes(&body, "button")[0];
    let request = json!({"revision": body["revision"], "edits": [
        {"op": "set-text", "target": launch["handle"], "value": "Begin 🧪"},
        {"op": "set-attr", "target": launch["handle"], "name": "disabled", "value": "false"}
    ]});
    let preview = edit(&app, request.clone(), false, 0);
    assert_eq!(preview["validated"], true);
    assert_eq!(preview["applied"], false);
    assert_eq!(source(&app), BUTTONS);
    let applied = edit(&app, request, true, 0);
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["newRevision"], preview["newRevision"]);
    let expected = BUTTONS
        .replacen(r#"text "Launch""#, r#"text "Begin 🧪""#, 1)
        .replace("disabled=busy", "disabled=false");
    assert_eq!(source(&app), expected);

    let stale = edit(
        &app,
        json!({"revision": body["revision"], "edits": [
            {"op": "set-text", "target": launch["handle"], "value": "Stale"}
        ]}),
        true,
        1,
    );
    assert_eq!(stale["applied"], false);
    assert_eq!(source(&app), expected);

    let current = inspect(&app, &["--component", "App"]);
    let launch = nodes(&current, "button")[0];
    let label = current["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["tag"] == "text" && node["parent"] == launch["handle"])
        .unwrap();
    let conflict = edit(
        &app,
        json!({"revision": current["revision"], "edits": [
            {"op": "set-text", "target": launch["handle"], "value": "Parent"},
            {"op": "set-text", "target": label["handle"], "value": "Child"}
        ]}),
        true,
        1,
    );
    assert_eq!(conflict["error"]["id"], "edit-conflict");
    assert_eq!(conflict["applied"], false);
    assert_eq!(source(&app), expected);
}

#[test]
fn text_edits_refuse_computed_templates_and_ambiguous_or_absent_labels_atomically() {
    let app = App::new("structural-text-refusals");
    let original = r#"component App
  state message = "Current"
  view
    column
      text "Keep" testId="untouched"
      text message testId="computed"
      text `Value ${message}` testId="template"
      button testId="dynamic-label"
        text message
      button testId="templated-label"
        text `Value ${message}`
      button testId="multiple"
        text "First"
        text "Second"
      button testId="empty" width=20 height=20 aria-label="Empty control"
      button testId="indirect"
        row
          text "Nested"
"#;
    contract::compile(original).unwrap();
    app.write("app.contract", original);
    for id in [
        "computed",
        "template",
        "dynamic-label",
        "templated-label",
        "multiple",
        "empty",
        "indirect",
    ] {
        let selector = format!("testId=\"{id}\"");
        let refused = response(
            cli(
                &app,
                &[
                    "edit",
                    "app.contract",
                    "--match",
                    "testId=\"untouched\"",
                    "--text",
                    "Must not apply",
                    "--set",
                    "aria-label=\"Must not apply either\"",
                    "--match",
                    &selector,
                    "--text",
                    "Replacement",
                    "--apply",
                ],
            ),
            1,
        );
        assert_eq!(refused["error"]["id"], "edit-text", "{id}: {refused}");
        assert_eq!(refused["applied"], false);
        assert_eq!(source(&app), original, "refused target: {id}");
    }
}
