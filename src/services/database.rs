use crate::model::{
    AgentKind, AppSession, DEFAULT_PROJECT_ICON, DEFAULT_TASK_ICON, ProjectTask, Repository,
    SessionState, TaskRepository, TaskSession, Workspace, WorkspaceColor, WorkspaceLayout,
};
use crate::paths::AppPaths;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
    session_path: PathBuf,
}

impl Database {
    pub fn open(paths: &AppPaths) -> Result<Self> {
        let connection = Connection::open(&paths.database)
            .with_context(|| format!("Unable to open {}", paths.database.display()))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS app_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY,
                sort_order INTEGER NOT NULL,
                payload TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS project_tasks (
                id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                sort_order INTEGER NOT NULL,
                payload TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS project_tasks_workspace_idx
                ON project_tasks(workspace_id, sort_order);
             CREATE TABLE IF NOT EXISTS local_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                aggregate_id TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL
             );",
        )?;

        let database = Self {
            connection: Arc::new(Mutex::new(connection)),
            session_path: paths.session.clone(),
        };
        database.import_legacy_if_empty(paths)?;
        Ok(database)
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare("SELECT payload FROM workspaces ORDER BY sort_order ASC, updated_at ASC")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| {
                let json = row?;
                serde_json::from_str(&json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn tasks(&self, workspace_id: Uuid) -> Result<Vec<ProjectTask>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT payload FROM project_tasks
             WHERE workspace_id = ?1 ORDER BY sort_order ASC, updated_at ASC",
        )?;
        statement
            .query_map([workspace_id.to_string()], |row| row.get::<_, String>(0))?
            .map(|row| {
                let json = row?;
                serde_json::from_str(&json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn all_tasks(&self) -> Result<Vec<ProjectTask>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT payload FROM project_tasks ORDER BY workspace_id, sort_order, updated_at",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| {
                let json = row?;
                serde_json::from_str(&json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn upsert_workspace(&self, workspace: &Workspace, sort_order: usize) -> Result<()> {
        let payload = serde_json::to_string(workspace)?;
        let event = serde_json::json!({ "workspace": workspace });
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO workspaces(id, sort_order, payload, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                sort_order = excluded.sort_order,
                payload = excluded.payload,
                updated_at = excluded.updated_at",
            params![
                workspace.id.to_string(),
                sort_order as i64,
                payload,
                workspace.updated_at.to_rfc3339()
            ],
        )?;
        insert_event(&transaction, "workspace.upserted", workspace.id, &event)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_workspace(&self, workspace_id: Uuid) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM project_tasks WHERE workspace_id = ?1",
            [workspace_id.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM workspaces WHERE id = ?1",
            [workspace_id.to_string()],
        )?;
        insert_event(
            &transaction,
            "workspace.removed",
            workspace_id,
            &serde_json::json!({ "id": workspace_id }),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_task(&self, task: &ProjectTask) -> Result<()> {
        let payload = serde_json::to_string(task)?;
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO project_tasks(id, workspace_id, sort_order, payload, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                sort_order = excluded.sort_order,
                payload = excluded.payload,
                updated_at = excluded.updated_at",
            params![
                task.id.to_string(),
                task.workspace_id.to_string(),
                task.sort_order,
                payload,
                task.updated_at.to_rfc3339()
            ],
        )?;
        insert_event(
            &transaction,
            "task.upserted",
            task.id,
            &serde_json::json!({ "task": task }),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn remove_task(&self, task_id: Uuid) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM project_tasks WHERE id = ?1",
            [task_id.to_string()],
        )?;
        insert_event(
            &transaction,
            "task.removed",
            task_id,
            &serde_json::json!({ "id": task_id }),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.connection
            .lock()
            .query_row("SELECT value FROM app_meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.connection.lock().execute(
            "INSERT INTO app_meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn load_session(&self) -> AppSession {
        fs::read(&self.session_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save_session(&self, session: &AppSession) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(session)?;
        let temporary = self.session_path.with_extension("json.tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &self.session_path)?;
        Ok(())
    }

    fn import_legacy_if_empty(&self, paths: &AppPaths) -> Result<()> {
        let current_count: i64 =
            self.connection
                .lock()
                .query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))?;
        if current_count > 0 || self.setting("legacy-import-completed")?.is_some() {
            return Ok(());
        }

        let Some(legacy_path) = legacy_database_path(paths) else {
            self.set_setting("legacy-import-completed", "missing")?;
            return Ok(());
        };
        let legacy = Connection::open_with_flags(&legacy_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("Unable to read legacy data at {}", legacy_path.display()))?;

        let workspaces = read_legacy_workspaces(&legacy)?;
        let tasks = read_legacy_tasks(&legacy)?;
        for (index, workspace) in workspaces.iter().enumerate() {
            self.upsert_workspace(workspace, index)?;
        }
        for task in &tasks {
            self.upsert_task(task)?;
        }
        if let Some(root) = legacy
            .query_row(
                "SELECT value FROM local_settings WHERE key = 'projects-root-path'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            self.set_setting("projects-root-path", &root)?;
        }
        self.set_setting("legacy-import-completed", &Utc::now().to_rfc3339())?;
        Ok(())
    }
}

fn insert_event(
    transaction: &rusqlite::Transaction<'_>,
    event_type: &str,
    aggregate_id: Uuid,
    payload: &serde_json::Value,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO local_events(event_type, aggregate_id, payload, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            event_type,
            aggregate_id.to_string(),
            serde_json::to_string(payload)?,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

fn legacy_database_path(paths: &AppPaths) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(parent) = paths.data_dir.parent() {
        candidates.push(
            parent
                .join("blackholes-desktop")
                .join("blackholes-local.db"),
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("blackholes-desktop")
                .join("blackholes-local.db"),
        );
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn read_legacy_workspaces(connection: &Connection) -> Result<Vec<Workspace>> {
    let mut repositories = std::collections::HashMap::<String, Vec<Repository>>::new();
    let mut repository_statement = connection.prepare(
        "SELECT id, workspace_id, name, path, branch
         FROM repositories ORDER BY workspace_id, sort_order",
    )?;
    let rows = repository_statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(1)?,
            Repository {
                id: parse_uuid(row.get::<_, String>(0)?),
                name: row.get(2)?,
                path: PathBuf::from(row.get::<_, String>(3)?),
                branch: row.get(4)?,
            },
        ))
    })?;
    for row in rows {
        let (workspace_id, repository) = row?;
        repositories
            .entry(workspace_id)
            .or_default()
            .push(repository);
    }

    let mut statement = connection.prepare(
        "SELECT id, name, display_name, icon, color, root_path, layout,
                ignored_repository_paths_json, created_at, updated_at
         FROM workspaces WHERE deleted_at IS NULL ORDER BY created_at",
    )?;
    let workspaces = statement
        .query_map([], |row| {
            let id_text: String = row.get(0)?;
            let ignored: Option<String> = row.get(7)?;
            Ok(Workspace {
                id: parse_uuid(id_text.clone()),
                name: row.get(1)?,
                display_name: row.get(2)?,
                icon: row
                    .get::<_, Option<String>>(3)?
                    .unwrap_or_else(|| DEFAULT_PROJECT_ICON.into()),
                color: parse_color(row.get::<_, Option<String>>(4)?.as_deref()),
                root_path: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
                layout: parse_layout(row.get::<_, Option<String>>(6)?.as_deref()),
                repositories: repositories.remove(&id_text).unwrap_or_default(),
                ignored_repository_paths: ignored
                    .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(PathBuf::from)
                    .collect(),
                created_at: parse_date(row.get(8)?),
                updated_at: parse_date(row.get(9)?),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(workspaces)
}

fn read_legacy_tasks(connection: &Connection) -> Result<Vec<ProjectTask>> {
    let mut repositories = std::collections::HashMap::<String, Vec<TaskRepository>>::new();
    let mut repository_statement = connection.prepare(
        "SELECT task_id, repository_id, worktree_path, branch, base_branch, base_revision,
                copy_local_changes, copy_environment_files, copied_environment_files_json,
                setup_command, prepared_at, added_at
         FROM project_task_repositories ORDER BY task_id, sort_order",
    )?;
    for row in repository_statement.query_map([], |row| {
        let copied_json: String = row.get(8)?;
        Ok((
            row.get::<_, String>(0)?,
            TaskRepository {
                repository_id: parse_uuid(row.get::<_, String>(1)?),
                worktree_path: PathBuf::from(row.get::<_, Option<String>>(2)?.unwrap_or_default()),
                branch: row
                    .get::<_, Option<String>>(3)?
                    .unwrap_or_else(|| "task".into()),
                base_branch: row.get(4)?,
                base_revision: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                copy_local_changes: row.get::<_, i64>(6)? != 0,
                copy_environment_files: row.get::<_, i64>(7)? != 0,
                copied_environment_files: serde_json::from_str(&copied_json).unwrap_or_default(),
                setup_command: row.get(9)?,
                prepared_at: row.get::<_, Option<String>>(10)?.map(parse_date),
                added_at: parse_date(row.get(11)?),
            },
        ))
    })? {
        let (task_id, repository) = row?;
        repositories.entry(task_id).or_default().push(repository);
    }

    let mut sessions = std::collections::HashMap::<String, Vec<TaskSession>>::new();
    let mut session_statement = connection.prepare(
        "SELECT id, task_id, repository_id, terminal_local_id, agent, label, state,
                created_at, updated_at, exited_at
         FROM project_task_sessions ORDER BY task_id, created_at",
    )?;
    for row in session_statement.query_map([], |row| {
        let id_text: String = row.get(0)?;
        let terminal_id: Option<String> = row.get(3)?;
        Ok((
            row.get::<_, String>(1)?,
            TaskSession {
                id: parse_uuid(id_text),
                repository_id: row.get::<_, Option<String>>(2)?.map(parse_uuid),
                terminal_local_id: terminal_id.map(parse_uuid).unwrap_or_else(Uuid::new_v4),
                agent: parse_agent(row.get::<_, Option<String>>(4)?.as_deref()),
                label: row.get(5)?,
                state: parse_session_state(&row.get::<_, String>(6)?),
                created_at: parse_date(row.get(7)?),
                updated_at: parse_date(row.get(8)?),
                exited_at: row.get::<_, Option<String>>(9)?.map(parse_date),
            },
        ))
    })? {
        let (task_id, session) = row?;
        sessions.entry(task_id).or_default().push(session);
    }

    let mut statement = connection.prepare(
        "SELECT id, workspace_id, title, description, icon, color, sort_order,
                worktree_root_path, created_at, updated_at
         FROM project_tasks WHERE deleted_at IS NULL ORDER BY workspace_id, sort_order",
    )?;
    let tasks = statement
        .query_map([], |row| {
            let id_text: String = row.get(0)?;
            Ok(ProjectTask {
                id: parse_uuid(id_text.clone()),
                workspace_id: parse_uuid(row.get::<_, String>(1)?),
                title: row.get(2)?,
                description: row.get(3)?,
                icon: row
                    .get::<_, Option<String>>(4)?
                    .unwrap_or_else(|| DEFAULT_TASK_ICON.into()),
                color: parse_color(row.get::<_, Option<String>>(5)?.as_deref()),
                sort_order: row.get(6)?,
                worktree_root_path: PathBuf::from(
                    row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                ),
                repositories: repositories.remove(&id_text).unwrap_or_default(),
                sessions: sessions.remove(&id_text).unwrap_or_default(),
                created_at: parse_date(row.get(8)?),
                updated_at: parse_date(row.get(9)?),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(tasks)
}

fn parse_uuid(value: String) -> Uuid {
    Uuid::parse_str(&value).unwrap_or_else(|_| Uuid::new_v4())
}

fn parse_date(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_layout(value: Option<&str>) -> WorkspaceLayout {
    match value {
        Some("single-repository") => WorkspaceLayout::SingleRepository,
        Some("multi-repository") => WorkspaceLayout::MultiRepository,
        _ => WorkspaceLayout::Empty,
    }
}

fn parse_color(value: Option<&str>) -> WorkspaceColor {
    match value {
        Some("coral") => WorkspaceColor::Coral,
        Some("peach") => WorkspaceColor::Peach,
        Some("amber") => WorkspaceColor::Amber,
        Some("sage") => WorkspaceColor::Sage,
        Some("mint") => WorkspaceColor::Mint,
        Some("sky") => WorkspaceColor::Sky,
        Some("lavender") => WorkspaceColor::Lavender,
        Some("rose") => WorkspaceColor::Rose,
        _ => WorkspaceColor::Slate,
    }
}

fn parse_agent(value: Option<&str>) -> AgentKind {
    match value {
        Some("codex") => AgentKind::Codex,
        Some("claude") => AgentKind::Claude,
        Some("gemini") => AgentKind::Gemini,
        _ => AgentKind::Shell,
    }
}

fn parse_session_state(value: &str) -> SessionState {
    match value {
        "restored" => SessionState::Restored,
        "working" => SessionState::Working,
        "attention" => SessionState::Attention,
        "exited" => SessionState::Exited,
        _ => SessionState::Idle,
    }
}
