use std::path::{Path, PathBuf};

use crate::hooks::{self, Hooks};
use crate::migrations::apply::AutoBackup;
use crate::migrations::options::MigrationConfig;
use crate::project::{self};

#[derive(Debug, Clone)]
pub struct Context {
    pub schema_dir: PathBuf,

    pub quiet: bool,

    /// Whether the project's hooks apply to the connection target.
    pub hooks_enabled: bool,

    pub project: Option<project::Context>,
    pub auto_backup: Option<AutoBackup>,
}

impl Context {
    pub async fn for_migration_config(
        cfg: &MigrationConfig,
        quiet: bool,
        hooks: Hooks,
        read_only: bool,
    ) -> anyhow::Result<Context> {
        let project = project::load_ctx(None, read_only).await?;

        let schema_dir = if let Some(schema_dir) = &cfg.schema_dir {
            schema_dir.clone()
        } else if let Some(project) = &project {
            project.resolve_schema_dir()?
        } else {
            let default_dir: PathBuf = "./dbschema".into();
            if !default_dir.exists() {
                anyhow::bail!(
                    "`dbschema` directory doesn't exist. Either create one, init a project or provide its path via --schema-dir."
                );
            }
            default_dir
        };

        let hooks_enabled = project
            .as_ref()
            .is_some_and(|project| hooks.applies_to(&project.location));

        Ok(Context {
            schema_dir,
            quiet,
            project,
            hooks_enabled,
            auto_backup: None,
        })
    }

    pub fn for_project(project: project::Context, hooks: Hooks) -> anyhow::Result<Context> {
        let schema_dir = project
            .manifest
            .project()
            .resolve_schema_dir(&project.location.root)?;

        Ok(Context {
            schema_dir,
            quiet: false,
            hooks_enabled: hooks.applies_to(&project.location),
            project: Some(project),
            auto_backup: None,
        })
    }

    /// Create a context for a temporary path.
    ///
    /// Hooks are skipped.
    pub fn for_temp_path(path: impl AsRef<Path>) -> anyhow::Result<Context> {
        Ok(Context {
            schema_dir: path.as_ref().to_path_buf(),
            quiet: false,
            hooks_enabled: false,
            project: None,
            auto_backup: None,
        })
    }

    /// Runs project hooks of the given action, if they apply.
    pub async fn run_hooks(&self, action: &'static str) -> anyhow::Result<()> {
        match &self.project {
            Some(project) if self.hooks_enabled => hooks::on_action(action, project).await,
            _ => Ok(()),
        }
    }

    pub fn with_auto_backup(&self, auto_backup: Option<AutoBackup>) -> Self {
        Self {
            auto_backup,
            ..self.clone()
        }
    }
}
