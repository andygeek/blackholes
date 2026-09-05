use anyhow::Result;
use gpui::{AssetSource, SharedString};
use gpui_component::IconNamed;
use std::borrow::Cow;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "app-icon.png" => Some(include_bytes!("../assets/app-icon.png")),
            "icons/chevron-down.svg" => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            "icons/chevron-right.svg" => Some(include_bytes!("../assets/icons/chevron-right.svg")),
            "icons/chevrons-up.svg" => Some(include_bytes!("../assets/icons/chevrons-up.svg")),
            "icons/claude-code.svg" => Some(include_bytes!("../assets/icons/claude-code.svg")),
            "icons/code-2.svg" => Some(include_bytes!("../assets/icons/code-2.svg")),
            "icons/codex.svg" => Some(include_bytes!("../assets/icons/codex.svg")),
            "icons/database.svg" => Some(include_bytes!("../assets/icons/database.svg")),
            "icons/ellipsis-vertical.svg" => {
                Some(include_bytes!("../assets/icons/ellipsis-vertical.svg"))
            }
            "icons/file.svg" => Some(include_bytes!("../assets/icons/file.svg")),
            "icons/folder.svg" => Some(include_bytes!("../assets/icons/folder.svg")),
            "icons/folder-open.svg" => Some(include_bytes!("../assets/icons/folder-open.svg")),
            "icons/git-branch.svg" => Some(include_bytes!("../assets/icons/git-branch.svg")),
            "icons/globe.svg" => Some(include_bytes!("../assets/icons/globe.svg")),
            "icons/layers-3.svg" => Some(include_bytes!("../assets/icons/layers-3.svg")),
            "icons/list-todo.svg" => Some(include_bytes!("../assets/icons/list-todo.svg")),
            "icons/pencil.svg" => Some(include_bytes!("../assets/icons/pencil.svg")),
            "icons/plus.svg" => Some(include_bytes!("../assets/icons/plus.svg")),
            "icons/refresh-cw.svg" => Some(include_bytes!("../assets/icons/refresh-cw.svg")),
            "icons/rocket.svg" => Some(include_bytes!("../assets/icons/rocket.svg")),
            "icons/search.svg" => Some(include_bytes!("../assets/icons/search.svg")),
            "icons/settings.svg" => Some(include_bytes!("../assets/icons/settings.svg")),
            "icons/square-terminal.svg" => {
                Some(include_bytes!("../assets/icons/square-terminal.svg"))
            }
            "icons/x.svg" => Some(include_bytes!("../assets/icons/x.svg")),
            _ => None,
        };

        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path.trim_matches('/') != "icons" {
            return Ok(Vec::new());
        }

        Ok(vec![
            "chevron-down.svg".into(),
            "chevron-right.svg".into(),
            "chevrons-up.svg".into(),
            "claude-code.svg".into(),
            "code-2.svg".into(),
            "codex.svg".into(),
            "database.svg".into(),
            "ellipsis-vertical.svg".into(),
            "file.svg".into(),
            "folder.svg".into(),
            "folder-open.svg".into(),
            "git-branch.svg".into(),
            "globe.svg".into(),
            "layers-3.svg".into(),
            "list-todo.svg".into(),
            "pencil.svg".into(),
            "plus.svg".into(),
            "refresh-cw.svg".into(),
            "rocket.svg".into(),
            "search.svg".into(),
            "settings.svg".into(),
            "square-terminal.svg".into(),
            "x.svg".into(),
        ])
    }
}

#[derive(Clone, Copy)]
pub enum AppIcon {
    ChevronDown,
    ChevronRight,
    ChevronsUp,
    ClaudeCode,
    Code2,
    Codex,
    Database,
    EllipsisVertical,
    File,
    Folder,
    FolderOpen,
    GitBranch,
    Globe,
    Layers3,
    ListTodo,
    Pencil,
    Plus,
    RefreshCw,
    Rocket,
    Search,
    Settings,
    SquareTerminal,
    X,
}

impl IconNamed for AppIcon {
    fn path(self) -> SharedString {
        match self {
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::ChevronsUp => "icons/chevrons-up.svg",
            Self::ClaudeCode => "icons/claude-code.svg",
            Self::Code2 => "icons/code-2.svg",
            Self::Codex => "icons/codex.svg",
            Self::Database => "icons/database.svg",
            Self::EllipsisVertical => "icons/ellipsis-vertical.svg",
            Self::File => "icons/file.svg",
            Self::Folder => "icons/folder.svg",
            Self::FolderOpen => "icons/folder-open.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::Globe => "icons/globe.svg",
            Self::Layers3 => "icons/layers-3.svg",
            Self::ListTodo => "icons/list-todo.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::Plus => "icons/plus.svg",
            Self::RefreshCw => "icons/refresh-cw.svg",
            Self::Rocket => "icons/rocket.svg",
            Self::Search => "icons/search.svg",
            Self::Settings => "icons/settings.svg",
            Self::SquareTerminal => "icons/square-terminal.svg",
            Self::X => "icons/x.svg",
        }
        .into()
    }
}
