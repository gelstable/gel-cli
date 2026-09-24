use std::path;

use gel_tokio::InstanceName;

use crate::print::{self, Highlight};
use crate::project;

/// Decides whether project hooks apply to a command.
///
/// Hooks belong to the project, so they only run when the command targets
/// the instance linked to that project.
#[derive(Debug, Clone)]
pub enum Hooks {
    /// Hooks were disabled with --skip-hooks or GEL_SKIP_HOOKS.
    Skip,
    /// The command connects to this instance (None for DSN, host, etc).
    Target(Option<InstanceName>),
}

impl Hooks {
    pub fn new(skip_hooks: bool, target: Option<InstanceName>) -> Hooks {
        if skip_hooks {
            Hooks::Skip
        } else {
            Hooks::Target(target)
        }
    }

    /// Returns true if hooks of the project at `location` should run.
    pub fn applies_to(&self, location: &project::Location) -> bool {
        match self {
            Hooks::Target(Some(target)) => project::is_linked_to(location, target),
            _ => false,
        }
    }
}

/// Runs project hooks of the given action.
/// Must not be called if --skip-hooks or GEL_SKIP_HOOKS is set.
#[tokio::main(flavor = "current_thread")]
pub async fn on_action_sync(
    action: &'static str,
    project: &project::Context,
) -> anyhow::Result<()> {
    on_action(action, project).await
}

/// Runs project hooks of the given action.
/// Must not be called if --skip-hooks or GEL_SKIP_HOOKS is set.
pub async fn on_action(action: &'static str, project: &project::Context) -> anyhow::Result<()> {
    let hooks = [
        project.manifest.hooks.as_ref(),
        project.manifest.hooks_extend.as_ref(),
    ];
    let scripts = hooks
        .into_iter()
        .flatten()
        .flat_map(|hooks| get_hook(action, hooks));
    let scripts: Vec<_> = if action.ends_with("after") {
        scripts.rev().collect()
    } else {
        scripts.collect()
    };
    for script in scripts {
        run_action(action, script, &project.location.root).await?;
    }
    Ok(())
}

async fn run_action<'m>(
    action: &'static str,
    script: &'m str,
    root_path: &'m path::Path,
) -> anyhow::Result<()> {
    print::msg!("{}", format!("hook {action}: {script}").muted());

    let status = crate::watch::run_script(action, script, root_path).await?;

    // abort on error
    if !status.success() {
        return Err(anyhow::anyhow!(
            "Hook {action} exited with status {status}."
        ));
    }
    Ok(())
}

fn get_hook<'m>(action: &'static str, hooks: &'m project::manifest::Hooks) -> Option<&'m str> {
    let hook = match action {
        "project.init.before" => &hooks.project.as_ref()?.init.as_ref()?.before,
        "project.init.after" => &hooks.project.as_ref()?.init.as_ref()?.after,
        "branch.switch.before" => &hooks.branch.as_ref()?.switch.as_ref()?.before,
        "branch.switch.after" => &hooks.branch.as_ref()?.switch.as_ref()?.after,
        "branch.wipe.before" => &hooks.branch.as_ref()?.wipe.as_ref()?.before,
        "branch.wipe.after" => &hooks.branch.as_ref()?.wipe.as_ref()?.after,
        "migration.apply.before" => &hooks.migration.as_ref()?.apply.as_ref()?.before,
        "migration.apply.after" => &hooks.migration.as_ref()?.apply.as_ref()?.after,
        "schema.update.before" => &hooks.schema.as_ref()?.update.as_ref()?.before,
        "schema.update.after" => &hooks.schema.as_ref()?.update.as_ref()?.after,
        "config.update.before" => &hooks.config.as_ref()?.update.as_ref()?.before,
        "config.update.after" => &hooks.config.as_ref()?.update.as_ref()?.after,
        _ => panic!("unknown action"),
    };
    hook.as_deref()
}
