use anyhow::{Context as _, Result, bail};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;
use walkdir::WalkDir;

pub const BLACKHOLES_SKILLS_PLUGIN_NAME: &str = "blackholes-skills";
const SKILL_FILE_NAME: &str = "SKILL.md";

#[derive(Clone, Debug)]
pub struct AgentSkill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct ImportSkillsReport {
    pub imported: Vec<AgentSkill>,
    pub errors: Vec<String>,
}

pub struct AgentSkillService;

impl AgentSkillService {
    pub fn list(plugin_root: &Path) -> Result<Vec<AgentSkill>> {
        let skills_root = plugin_root.join("skills");
        fs::create_dir_all(&skills_root)
            .with_context(|| format!("Unable to create {}", skills_root.display()))?;
        let mut skills = fs::read_dir(&skills_root)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| parse_skill(&entry.path()).ok())
            .collect::<Vec<_>>();
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(skills)
    }

    /// Import either one skill directory or every direct child skill in a
    /// collection directory. Supporting files are copied alongside SKILL.md.
    pub fn import(source: &Path, plugin_root: &Path) -> Result<ImportSkillsReport> {
        let candidates = skill_candidates(source)?;
        if candidates.is_empty() {
            bail!(
                "{} does not contain a SKILL.md or direct child skill directories",
                source.display()
            );
        }

        let skills_root = plugin_root.join("skills");
        fs::create_dir_all(&skills_root)?;
        let mut report = ImportSkillsReport::default();
        let mut imported_names = HashSet::new();
        for candidate in candidates {
            match import_one(&candidate, &skills_root) {
                Ok(skill) if imported_names.insert(skill.name.clone()) => {
                    report.imported.push(skill)
                }
                Ok(_) => {}
                Err(error) => report
                    .errors
                    .push(format!("{}: {error:#}", candidate.display())),
            }
        }
        report
            .imported
            .sort_by(|left, right| left.name.cmp(&right.name));
        Ok(report)
    }
}

fn skill_candidates(source: &Path) -> Result<Vec<PathBuf>> {
    if source.join(SKILL_FILE_NAME).is_file() {
        return Ok(vec![source.to_path_buf()]);
    }
    let mut candidates = fs::read_dir(source)
        .with_context(|| format!("Unable to read {}", source.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(SKILL_FILE_NAME).is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    Ok(candidates)
}

fn import_one(source: &Path, skills_root: &Path) -> Result<AgentSkill> {
    let skill = parse_skill(source)?;
    let temporary = skills_root.join(format!(".import-{}-{}", skill.name, Uuid::new_v4()));
    copy_skill_directory(source, &temporary)?;

    let destination = skills_root.join(&skill.name);
    let backup = skills_root.join(format!(".backup-{}-{}", skill.name, Uuid::new_v4()));
    if destination.exists() {
        fs::rename(&destination, &backup).with_context(|| {
            format!(
                "Unable to prepare the existing imported skill {}",
                destination.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        if backup.exists() {
            let _ = fs::rename(&backup, &destination);
        }
        let _ = fs::remove_dir_all(&temporary);
        return Err(error)
            .with_context(|| format!("Unable to install the skill at {}", destination.display()));
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }

    parse_skill(&destination)
}

fn copy_skill_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in WalkDir::new(source) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if entry.file_type().is_symlink() {
            bail!("symbolic links are not supported in imported skills")
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn parse_skill(path: &Path) -> Result<AgentSkill> {
    let skill_file = path.join(SKILL_FILE_NAME);
    let contents = fs::read_to_string(&skill_file)
        .with_context(|| format!("Unable to read {}", skill_file.display()))?;
    let frontmatter = yaml_frontmatter(&contents)?;
    let name =
        frontmatter_value(frontmatter, "name").context("SKILL.md frontmatter is missing name")?;
    validate_skill_name(&name)?;
    let description = frontmatter_value(frontmatter, "description")
        .context("SKILL.md frontmatter is missing description")?;
    if description.is_empty() {
        bail!("SKILL.md description cannot be empty")
    }
    Ok(AgentSkill {
        name,
        description,
        path: path.to_path_buf(),
    })
}

fn yaml_frontmatter(contents: &str) -> Result<&str> {
    let rest = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))
        .context("SKILL.md must start with YAML frontmatter")?;
    let end = rest
        .find("\n---")
        .context("SKILL.md frontmatter is not closed")?;
    Ok(&rest[..end])
}

fn frontmatter_value(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    frontmatter.lines().find_map(|line| {
        let value = line.strip_prefix(&prefix)?.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        Some(value.to_string())
    })
}

fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        bail!("skill name must contain between 1 and 64 characters")
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        bail!("skill name may only contain lowercase letters, numbers, and hyphens")
    }
    Ok(())
}
