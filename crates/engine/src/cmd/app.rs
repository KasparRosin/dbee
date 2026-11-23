//! `app.checkForUpdates`, `app.update` and `app.setUpdateCheck`: the update check and the update
//! button's action.
//!
//! - `app.checkForUpdates {}` asks for the newest release now, as a job, and answers
//!   `{checked: true, current, latest, available, update}`; its failure is the job's error (code
//!   `update`). It is disabled in a run that never checks (a test, an isolated run, a development
//!   build), and `busy` while a check runs.
//! - `app.update {}` does what the update button does. For an offer that opens the release's
//!   page it answers `{action: "openPage", url}` and opens nothing: only the window, for a click
//!   on its button, opens the page. For an offer dbee installs itself it starts the install as a
//!   job, whose answer is `{action: "install", update}`, or an error (code `update`) saying why
//!   it failed, the state then `failed`, from where the command runs the install again; once the
//!   release is installed it answers `{action: "restart"}`.
//! - `app.setUpdateCheck {enabled}` turns the automatic check on or off and keeps the choice in
//!   `settings.json`; `{enabled, note?}`, `note` saying why the choice could not be kept.

use dbee_update::Action;
use serde_json::json;

use super::{CommandSpec, always, bad, disabled, p};
use crate::update::{CHECK, UPDATE, UpdateState, action_json};
use crate::{EngineError, Json, Outcome, Session};

const SET_CHECK: &str = "app.setUpdateCheck";

/// Why a run without an update checker cannot check.
pub const CHECKS_OFF: &str = "update checks are off in this run";

/// Why `app.update` cannot run with no offer.
pub const NO_UPDATE: &str = "no update is available";

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: CHECK,
        label: "Check for Updates",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: can_check,
        run: check,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: UPDATE,
        label: "Update",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: can_update,
        run: update,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: SET_CHECK,
        label: "Check for Updates Automatically",
        menu: &[],
        shortcut: None,
        params: "{enabled}",
        enabled: always,
        run: set_check,
        journal: true,
        undoable: false,
    },
];

fn can_check(s: &Session) -> Result<(), String> {
    if s.updates.is_some() { Ok(()) } else { Err(CHECKS_OFF.into()) }
}

fn can_update(s: &Session) -> Result<(), String> {
    if s.state.update == UpdateState::None { Err(NO_UPDATE.into()) } else { Ok(()) }
}

fn check(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    if s.jobs.is_running_label(CHECK) {
        return Err(EngineError::Busy("an update check is running".into()));
    }
    s.start_update_check(false).map(Outcome::Started).ok_or_else(|| disabled(CHECKS_OFF))
}

fn update(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let offer = match &s.state.update {
        UpdateState::None => return Err(disabled(NO_UPDATE)),
        UpdateState::Updating(_) => return Err(EngineError::Busy("the update is being installed".into())),
        UpdateState::Ready(_) => return Ok(Outcome::Done(json!({ "action": "restart" }))),
        UpdateState::Available(offer) | UpdateState::Failed { offer, .. } => offer.clone(),
    };
    match &offer.action {
        Action::OpenPage { url } => Ok(Outcome::Done(json!({ "action": "openPage", "url": url }))),
        Action::Install { .. } => {
            let action = action_json(&offer.action);
            s.start_update_install(offer).map(Outcome::Started).ok_or_else(|| disabled(format!("this copy has no install step for {action}")))
        }
    }
}

fn set_check(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    if params.get("enabled").is_none_or(Json::is_null) {
        return Err(bad("`enabled` is required (true or false)"));
    }
    let enabled = p::bool_or(params, "enabled", true)?;
    let was = s.update_check_enabled;
    s.update_check_enabled = enabled;
    if enabled && !was {
        s.update_check_soon();
    }
    let mut result = json!({ "enabled": enabled });
    if let Err(e) = dbee_config::save_update_check(s.config_dir.as_deref(), enabled) {
        result["note"] = json!(e.to_string());
    }
    Ok(Outcome::Done(result))
}
