//! The race-engineer skill, embedded so a standalone `vr` binary can install it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Subcommand, ValueEnum};

pub const SKILL_NAME: &str = "race-engineer";

/// (relative path, contents)
pub const FILES: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../../skills/race-engineer/SKILL.md"),
    ),
    (
        "references/commands.md",
        include_str!("../../../skills/race-engineer/references/commands.md"),
    ),
    (
        "references/radio-style.md",
        include_str!("../../../skills/race-engineer/references/radio-style.md"),
    ),
    (
        "references/strategy.md",
        include_str!("../../../skills/race-engineer/references/strategy.md"),
    ),
    (
        "references/setup.md",
        include_str!("../../../skills/race-engineer/references/setup.md"),
    ),
    (
        "references/glossary.md",
        include_str!("../../../skills/race-engineer/references/glossary.md"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// Claude Code (`~/.claude/skills`, or `.claude/skills` with --project).
    Claude,
    /// Codex (`~/.agents/skills`, or `.agents/skills` with --project).
    Codex,
    /// Both.
    All,
}

#[derive(Debug, Subcommand)]
pub enum SkillCmd {
    /// Install (or update) the skill for Claude Code and/or Codex.
    Install {
        #[arg(long, value_enum, default_value_t = Target::All)]
        target: Target,
        /// Install into the current project instead of the user directory.
        #[arg(long)]
        project: bool,
        /// Install into this skills directory instead (the skill goes in <dir>/race-engineer).
        #[arg(long, conflicts_with_all = ["target", "project"])]
        dir: Option<PathBuf>,
    },
    /// Print SKILL.md.
    Show,
    /// Print where the skill would be installed.
    Paths {
        #[arg(long)]
        project: bool,
    },
}

pub fn skills_dirs(target: Target, project: bool) -> Vec<(&'static str, PathBuf)> {
    let base = if project {
        std::env::current_dir().unwrap_or_default()
    } else {
        dirs::home_dir().unwrap_or_default()
    };
    let mut out = Vec::new();
    if matches!(target, Target::Claude | Target::All) {
        out.push(("claude", base.join(".claude").join("skills")));
    }
    if matches!(target, Target::Codex | Target::All) {
        out.push(("codex", base.join(".agents").join("skills")));
    }
    out
}

fn install_into(skills_dir: &Path) -> std::io::Result<PathBuf> {
    let root = skills_dir.join(SKILL_NAME);
    for (rel, body) in FILES {
        let p = root.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, body)?;
    }
    Ok(root)
}

pub fn run(cmd: SkillCmd) -> anyhow::Result<ExitCode> {
    match cmd {
        SkillCmd::Install {
            target,
            project,
            dir,
        } => {
            let dirs: Vec<(&str, PathBuf)> = match dir {
                Some(d) => vec![("custom", d)],
                None => skills_dirs(target, project),
            };
            for (who, d) in dirs {
                let root = install_into(&d)?;
                println!("installed {SKILL_NAME} for {who}: {}", root.display());
            }
        }
        SkillCmd::Show => print!("{}", FILES[0].1),
        SkillCmd::Paths { project } => {
            for (who, d) in skills_dirs(Target::All, project) {
                let root = d.join(SKILL_NAME);
                let state = if root.join("SKILL.md").exists() {
                    "installed"
                } else {
                    "not installed"
                };
                println!("{who}: {} ({state})", root.display());
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
