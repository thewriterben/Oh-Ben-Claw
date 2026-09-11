//! The curator: learned skills must earn their context rent (parity item 4,
//! 2026-09-11).
//!
//! Every enabled skill is a tool schema in every prompt. Left alone, the
//! self-improvement loop grows the set monotonically — on the bench it had
//! produced three skills for one `time /T` recipe under three phrasings of the
//! same question. The curator runs after each improvement pass and:
//!
//! 1. **deduplicates** — enabled learned skills with the same recipe (same
//!    kind, tool and arguments) keep the most-used one and disable the rest;
//! 2. **archives** — a learned skill not used for `archive_after_days` (from
//!    its last use, or from when it was installed if never used) is disabled;
//! 3. **caps** — at most `max_enabled_learned` learned skills stay enabled,
//!    the least used and oldest going first;
//! 4. **documents** — every learned skill gets a `SKILL.md` beside its
//!    manifest in the agentskills.io layout (YAML front matter with `name` and
//!    `description`, then Markdown), so a skill can be read, shared or ported
//!    without decoding the JSON.
//!
//! Disabling is a manifest rewrite (`enabled = false` plus a `curated:…` tag
//! saying why); nothing is deleted, and `skill_forge install` or the CLI
//! re-enables. Operator-authored skills (no `learned` tag, name not
//! `learned_…`) and skills tagged `curated:pinned` are never touched.

use super::usage::UsageLedger;
use super::{SkillForge, SkillKind, SkillManifest};
use std::collections::BTreeMap;

/// The knobs, from `[self_improvement]`.
#[derive(Debug, Clone)]
pub struct CuratorPolicy {
    /// Disable a learned skill unused for this many days. 0 = never.
    pub archive_after_days: u64,
    /// Keep at most this many learned skills enabled. 0 = no cap.
    pub max_enabled_learned: usize,
}

impl Default for CuratorPolicy {
    fn default() -> Self {
        Self {
            archive_after_days: 30,
            max_enabled_learned: 40,
        }
    }
}

/// What one pass did.
#[derive(Debug, Default, Clone)]
pub struct CuratorReport {
    pub duplicates_disabled: Vec<String>,
    pub stale_disabled: Vec<String>,
    pub over_cap_disabled: Vec<String>,
    pub skill_md_written: usize,
}

impl CuratorReport {
    /// Whether the tool registry needs a resync.
    pub fn changed_registry(&self) -> bool {
        !(self.duplicates_disabled.is_empty()
            && self.stale_disabled.is_empty()
            && self.over_cap_disabled.is_empty())
    }
}

pub const TAG_LEARNED: &str = "learned";
pub const TAG_PINNED: &str = "curated:pinned";

/// Whether the curator may touch this manifest.
pub fn is_learned(m: &SkillManifest) -> bool {
    (m.name.starts_with("learned_") || m.tags.iter().any(|t| t == TAG_LEARNED))
        && !m.tags.iter().any(|t| t == TAG_PINNED)
}

/// A key that is equal for two skills that do the same thing: the kind and
/// everything inside it, serialised canonically (serde_json sorts map keys
/// when `preserve_order` is off, which it is here).
pub fn recipe_key(kind: &SkillKind) -> String {
    serde_json::to_string(kind).unwrap_or_default()
}

const DAY_MS: u64 = 86_400_000;

/// One pass. `now_ms` is injected so the age rules are testable.
pub fn curate(
    forge: &SkillForge,
    usage: &UsageLedger,
    policy: &CuratorPolicy,
    now_ms: u64,
) -> anyhow::Result<CuratorReport> {
    let mut report = CuratorReport::default();
    let manifests = forge.list_manifests()?;

    // The reference time for "unused since": last use, else first seen, else
    // the manifest file's modification time (an install without any use yet).
    let reference = |m: &SkillManifest| -> u64 {
        if let Some(u) = usage.get(&m.name) {
            return u.last_used_ms.max(u.first_seen_ms);
        }
        forge
            .manifest_path(&m.name)
            .metadata()
            .and_then(|md| md.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(now_ms)
    };
    let uses = |m: &SkillManifest| usage.get(&m.name).map(|u| u.count).unwrap_or(0);

    let mut enabled: Vec<SkillManifest> = manifests
        .iter()
        .filter(|m| m.enabled && is_learned(m))
        .cloned()
        .collect();

    // 1. Duplicates: same recipe, keep the most used (then the shortest name).
    let mut by_recipe: BTreeMap<String, Vec<SkillManifest>> = BTreeMap::new();
    for m in enabled.drain(..) {
        by_recipe.entry(recipe_key(&m.kind)).or_default().push(m);
    }
    for (_, mut group) in by_recipe {
        group.sort_by(|a, b| {
            uses(b)
                .cmp(&uses(a))
                .then(a.name.len().cmp(&b.name.len()))
                .then(a.name.cmp(&b.name))
        });
        let keep = group.remove(0);
        for dup in group {
            disable(forge, &dup, &format!("curated:duplicate-of:{}", keep.name))?;
            report.duplicates_disabled.push(dup.name);
        }
        enabled.push(keep);
    }

    // 2. Stale.
    if policy.archive_after_days > 0 {
        let cutoff = now_ms.saturating_sub(policy.archive_after_days * DAY_MS);
        let mut kept = Vec::new();
        for m in enabled.drain(..) {
            if reference(&m) < cutoff {
                disable(forge, &m, "curated:stale")?;
                report.stale_disabled.push(m.name);
            } else {
                kept.push(m);
            }
        }
        enabled = kept;
    }

    // 3. Cap: least used first, then oldest.
    if policy.max_enabled_learned > 0 && enabled.len() > policy.max_enabled_learned {
        enabled.sort_by(|a, b| {
            uses(b)
                .cmp(&uses(a))
                .then(reference(b).cmp(&reference(a)))
                .then(a.name.cmp(&b.name))
        });
        for m in enabled.split_off(policy.max_enabled_learned) {
            disable(forge, &m, "curated:over-cap")?;
            report.over_cap_disabled.push(m.name);
        }
    }

    // 4. SKILL.md for every learned skill, enabled or not.
    for m in forge.list_manifests()? {
        if (is_learned(&m) || m.tags.iter().any(|t| t == TAG_PINNED)) && forge.write_skill_md(&m)? {
            report.skill_md_written += 1;
        }
    }

    if report.changed_registry() {
        tracing::info!(
            duplicates = report.duplicates_disabled.len(),
            stale = report.stale_disabled.len(),
            over_cap = report.over_cap_disabled.len(),
            "skill curator disabled learned skills"
        );
    }
    Ok(report)
}

fn disable(forge: &SkillForge, m: &SkillManifest, tag: &str) -> anyhow::Result<()> {
    let mut m = m.clone();
    m.enabled = false;
    m.tags.retain(|t| !t.starts_with("curated:"));
    m.tags.push(tag.to_string());
    forge.install_skill(&m)?;
    tracing::info!(skill = %m.name, why = tag, "skill curator disabled a learned skill");
    Ok(())
}

/// The agentskills.io layout: YAML front matter, then Markdown a person can
/// read. The JSON manifest stays the executable source of truth.
pub fn render_skill_md(m: &SkillManifest) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("name: {}\n", m.name));
    s.push_str(&format!("description: {}\n", yaml_scalar(&m.description)));
    s.push_str("---\n\n");
    s.push_str(&format!("# {}\n\n{}\n\n", m.name, m.description.trim()));
    s.push_str("## How it runs\n\n");
    match &m.kind {
        SkillKind::Shell { command } => {
            s.push_str(&format!("Shell command:\n\n```\n{command}\n```\n"));
        }
        SkillKind::Http {
            url,
            method,
            headers: _,
            body_template,
        } => {
            s.push_str(&format!("HTTP `{method}` to `{url}`"));
            match body_template {
                Some(b) => s.push_str(&format!(" with body:\n\n```\n{b}\n```\n")),
                None => s.push('\n'),
            }
        }
        SkillKind::Delegate { tool, fixed_args } => {
            s.push_str(&format!(
                "Calls the `{tool}` tool with fixed arguments:\n\n```json\n{}\n```\n",
                serde_json::to_string_pretty(fixed_args).unwrap_or_default()
            ));
        }
        SkillKind::Sequence { steps } => {
            s.push_str("A sequence of tool calls:\n\n");
            for (i, step) in steps.iter().enumerate() {
                s.push_str(&format!(
                    "{}. `{}` with `{}`\n",
                    i + 1,
                    step.tool,
                    serde_json::to_string(&step.args).unwrap_or_default()
                ));
            }
        }
    }
    s.push_str(&format!(
        "\nStage: `{}` · enabled: `{}` · version: `{}` · tags: {}\n",
        serde_json::to_value(m.stage)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default(),
        m.enabled,
        m.version.clone().unwrap_or_default(),
        if m.tags.is_empty() {
            "none".to_string()
        } else {
            m.tags
                .iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    if m.parameters
        .get("properties")
        .and_then(|p| p.as_object())
        .is_some_and(|o| !o.is_empty())
    {
        s.push_str(&format!(
            "\n## Parameters\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&m.parameters).unwrap_or_default()
        ));
    }
    s
}

fn yaml_scalar(text: &str) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "\"{}\"",
        one_line.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn learned(name: &str, kind: SkillKind) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: format!("Learned from a successful run: {name}"),
            kind,
            parameters: json!({"type": "object", "properties": {}}),
            version: Some("0.1.0-learned".into()),
            stage: Default::default(),
            tags: vec![TAG_LEARNED.into()],
            enabled: true,
            timeout_secs: 30,
        }
    }

    fn time_recipe() -> SkillKind {
        SkillKind::Delegate {
            tool: "shell".into(),
            fixed_args: json!({"command": "time /T", "timeout_secs": 10}),
        }
    }

    fn setup() -> (tempfile::TempDir, SkillForge, UsageLedger) {
        let dir = tempfile::tempdir().unwrap();
        let forge = SkillForge::new(dir.path().join("skills"));
        let usage = UsageLedger::load(dir.path().join("usage.json"));
        (dir, forge, usage)
    }

    #[test]
    fn duplicates_keep_the_most_used_then_the_shortest_name() {
        let (_d, forge, usage) = setup();
        forge
            .install_skill(&learned(
                "learned_what_time_is_it_long_phrasing",
                time_recipe(),
            ))
            .unwrap();
        forge
            .install_skill(&learned("learned_time_now", time_recipe()))
            .unwrap();
        forge
            .install_skill(&learned("learned_clock", time_recipe()))
            .unwrap();
        forge
            .install_skill(&learned(
                "learned_disk",
                SkillKind::Shell {
                    command: "dir".into(),
                },
            ))
            .unwrap();
        let mut operator = learned("nightly_backup", time_recipe());
        operator.tags.clear();
        forge.install_skill(&operator).unwrap();
        usage.record_at("learned_time_now", 10);
        usage.record_at("learned_time_now", 20);

        let rep = curate(
            &forge,
            &usage,
            &CuratorPolicy {
                archive_after_days: 0,
                max_enabled_learned: 0,
            },
            1_000,
        )
        .unwrap();
        let mut dups = rep.duplicates_disabled.clone();
        dups.sort();
        assert_eq!(
            dups,
            ["learned_clock", "learned_what_time_is_it_long_phrasing"]
        );
        let enabled: Vec<String> = forge
            .load_all()
            .unwrap()
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        assert!(enabled.contains(&"learned_time_now".to_string()));
        assert!(enabled.contains(&"learned_disk".to_string()));
        assert!(
            enabled.contains(&"nightly_backup".to_string()),
            "operator skills are never touched"
        );
        let clock = forge
            .list_manifests()
            .unwrap()
            .into_iter()
            .find(|m| m.name == "learned_clock")
            .unwrap();
        assert!(!clock.enabled);
        assert!(clock
            .tags
            .contains(&"curated:duplicate-of:learned_time_now".to_string()));
        assert_eq!(
            rep.skill_md_written, 0,
            "install_skill already wrote every SKILL.md"
        );
        assert!(forge.skill_dir.join("learned_disk.SKILL.md").exists());
        std::fs::remove_file(forge.skill_dir.join("learned_disk.SKILL.md")).unwrap();
        let rep = curate(
            &forge,
            &usage,
            &CuratorPolicy {
                archive_after_days: 0,
                max_enabled_learned: 0,
            },
            1_000,
        )
        .unwrap();
        assert_eq!(rep.skill_md_written, 1, "a missing SKILL.md is restored");
        assert!(forge.skill_dir.join("learned_clock.SKILL.md").exists());
        // A second pass changes nothing and rewrites nothing.
        let rep = curate(
            &forge,
            &usage,
            &CuratorPolicy {
                archive_after_days: 0,
                max_enabled_learned: 0,
            },
            1_000,
        )
        .unwrap();
        assert!(!rep.changed_registry());
        assert_eq!(rep.skill_md_written, 0);
    }

    #[test]
    fn stale_and_over_cap_skills_are_disabled_in_that_order() {
        let (_d, forge, usage) = setup();
        for i in 0..4 {
            forge
                .install_skill(&learned(
                    &format!("learned_s{i}"),
                    SkillKind::Shell {
                        command: format!("echo {i}"),
                    },
                ))
                .unwrap();
        }
        let day = 86_400_000u64;
        let now = 100 * day;
        usage.record_at("learned_s0", now - 40 * day); // stale
        usage.record_at("learned_s1", now - day); // fresh, used once
        for _ in 0..5 {
            usage.record_at("learned_s2", now - 2 * day); // fresh, used five times
        }
        usage.record_at("learned_s3", now - 3 * day); // fresh, used once, older
        let rep = curate(
            &forge,
            &usage,
            &CuratorPolicy {
                archive_after_days: 30,
                max_enabled_learned: 2,
            },
            now,
        )
        .unwrap();
        assert_eq!(rep.stale_disabled, ["learned_s0"]);
        assert_eq!(
            rep.over_cap_disabled,
            ["learned_s3"],
            "least used, oldest goes first"
        );
        let m = forge.list_manifests().unwrap();
        assert!(m
            .iter()
            .find(|m| m.name == "learned_s0")
            .unwrap()
            .tags
            .contains(&"curated:stale".into()));
        assert!(m
            .iter()
            .find(|m| m.name == "learned_s3")
            .unwrap()
            .tags
            .contains(&"curated:over-cap".into()));
        assert!(m.iter().find(|m| m.name == "learned_s2").unwrap().enabled);
    }

    #[test]
    fn a_pinned_skill_is_left_alone() {
        let (_d, forge, usage) = setup();
        let mut pinned = learned("learned_keep", time_recipe());
        pinned.tags.push(TAG_PINNED.into());
        forge.install_skill(&pinned).unwrap();
        forge
            .install_skill(&learned("learned_dup", time_recipe()))
            .unwrap();
        let rep = curate(&forge, &usage, &CuratorPolicy::default(), 10 * 86_400_000).unwrap();
        assert!(
            rep.duplicates_disabled.is_empty(),
            "the pinned one is not in the pool, so no duplicate pair"
        );
        assert!(forge.list_manifests().unwrap().iter().all(|m| m.enabled));
    }

    #[test]
    fn skill_md_is_the_agentskills_layout() {
        let m = learned("learned_time_now", time_recipe());
        let md = render_skill_md(&m);
        assert!(md.starts_with("---\nname: learned_time_now\ndescription: \"Learned from a successful run: learned_time_now\"\n---\n\n# learned_time_now\n"));
        assert!(md.contains("Calls the `shell` tool with fixed arguments:"));
        assert!(md.contains("\"command\": \"time /T\""));
        assert!(md.contains("Stage: `autonomous` · enabled: `true`"));
        assert!(!md.contains("## Parameters"), "no parameters, no section");
    }
}
