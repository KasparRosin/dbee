//! The schema tree driven through the control channel, one frame at a time in a kittest harness
//! over [`DbeeApp::ui`]: copying a table's name (`clipboard.copyName`). Assertions read
//! `ui.inspect`, `document.inspect` and the text the frames put on the clipboard.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is only read.

mod common;

use common::*;
use serde_json::json;

#[test]
fn copy_name_copies_the_bare_name() {
    let db = dbee_testkit::require_testdb!();
    for (schema, table) in [("shop", "orders"), ("odd", "Other Ödd")] {
        // A window each: a second notice waits behind the first.
        let mut ui = Ui::connected(db);
        let done = ui.execute("clipboard.copyName", json!({ "schema": schema, "table": table }));
        assert_eq!(done, json!({ "text": table, "bytes": table.len() }), "{schema}.{table}");
        ui.frame();
        assert_eq!(ui.copied().as_deref(), Some(table), "the name as written, without its schema");
        assert_eq!(ui.value("notice"), json!(format!("Copied {table}")));
    }
}

#[test]
fn copy_name_needs_a_table_the_catalog_has() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    let call = |ui: &mut Ui, params| ui.call("engine.execute", json!({ "command": "clipboard.copyName", "params": params }));
    let r = call(&mut ui, json!({}));
    assert_eq!(r["code"], "badParams", "{r}");
    let r = call(&mut ui, json!({ "schema": "shop", "table": "nope" }));
    assert_eq!(r["code"], "badParams", "{r}");
    assert!(r.to_string().contains("shop.nope"), "the error names the table: {r}");
    ui.frame();
    assert!(ui.inspect()["dialog"].is_null(), "no dialog opened");
    assert_eq!(ui.copied(), None, "nothing copied");
}

#[test]
fn copy_name_without_a_connection_is_disabled() {
    let mut ui = Ui::fresh(egui::vec2(1400.0, 900.0));
    let r = ui.call("engine.execute", json!({ "command": "clipboard.copyName", "params": { "schema": "shop", "table": "orders" } }));
    assert_eq!(r["code"], "disabled", "{r}");
    assert_eq!(r["data"]["why"], "not connected", "{r}");
    let commands = ui.ok("engine.commands", json!({}));
    let c = commands.as_array().unwrap().iter().find(|c| c["id"] == "clipboard.copyName").cloned().unwrap_or_default();
    assert_eq!((c["menu"].clone(), c["enabled"].clone()), (json!([]), json!(false)), "{c}");
}
