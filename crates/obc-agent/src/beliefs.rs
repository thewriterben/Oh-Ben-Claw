//! Belief commands from chat (personal-agents item 5, 2026-11-02).
//!
//! Two things the operator can say on any chat channel that are answered from
//! world memory, with no model turn:
//!
//! - **"why do you believe X"** (or "why do you think X") looks the belief up
//!   and says where it came from: which source said so and when, what kind of
//!   claim it is (a reading, a computation, an agent's claim, something a
//!   person said), whether what it rests on still stands, and what rests on it.
//! - **"forget that"** (after a lookup) or **"forget <entity>"** withdraws it
//!   with an operator closure in `world.rs`: the row stays in history, marked
//!   as withdrawn on request, and stops being current state. A newer report
//!   about the same thing is believed again.
//!
//! Anything that does not resolve to a belief in world memory ("why do you
//! think it will rain", "forget that" with nothing looked up, "forget the
//! milk") goes on to the model as an ordinary message. Only attended turns are
//! intercepted: a scheduled task's or an A2A caller's text is not the operator
//! speaking.

use crate::{Agent, AgentResponse};
use anyhow::Result;
use obc_memory::world::{Closure, Fact, Origin, Support, WorldMemory};
use obc_providers::ChatRole;

/// A belief command, as parsed from a chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// "why do you believe the front door is locked"
    Why(String),
    /// "forget that", "forget it"
    ForgetThat,
    /// "forget front_door.lock": acted on only for an exact entity.
    Forget(String),
}

/// Parse a chat message as a belief command. `None` for anything else.
pub fn parse(msg: &str) -> Option<Command> {
    let m = msg
        .trim()
        .trim_end_matches(['?', '.', '!', ' '])
        .to_lowercase();
    for lead in [
        "why do you believe ",
        "why do you think ",
        "why do you say ",
    ] {
        if let Some(rest) = m.strip_prefix(lead) {
            let rest = rest.strip_prefix("that ").unwrap_or(rest).trim();
            if rest.is_empty() || matches!(rest, "that" | "this" | "it") {
                return None;
            }
            return Some(Command::Why(rest.to_string()));
        }
    }
    let rest = m.strip_prefix("forget ")?.trim();
    match rest {
        "that" | "this" | "it" | "that belief" | "this belief" => Some(Command::ForgetThat),
        "" => None,
        _ => Some(Command::Forget(rest.to_string())),
    }
}

/// What a command came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    /// The entity a following "forget that" means, when this was a lookup of one.
    pub referent: Option<String>,
}

/// Answer `cmd` from `world`. `Ok(None)` means it is not about a belief held
/// there, and the message goes to the model instead.
pub fn answer(
    world: &WorldMemory,
    cmd: &Command,
    referent: Option<&str>,
    session: &str,
    now_ms: u64,
) -> Result<Option<Answer>> {
    match cmd {
        Command::Why(subject) => {
            let open = world.open_facts_with_prefix("")?;
            match find(&open, subject) {
                Found::None => Ok(None),
                Found::One(f) => Ok(Some(Answer {
                    text: explain(world, f, now_ms)?,
                    referent: Some(f.entity.clone()),
                })),
                Found::Many(fs) => {
                    let mut t = String::from("More than one belief could be meant:\n");
                    for f in &fs {
                        t.push_str(&format!(
                            "- {} = {} (#{})\n",
                            f.entity,
                            show(&f.value),
                            f.id
                        ));
                    }
                    t.push_str("Ask about one by name, e.g. \"why do you believe ");
                    t.push_str(&fs[0].entity);
                    t.push('"');
                    Ok(Some(Answer {
                        text: t,
                        referent: None,
                    }))
                }
            }
        }
        Command::ForgetThat => match referent {
            Some(entity) => forget(world, entity, session, now_ms),
            None => Ok(None),
        },
        Command::Forget(name) => {
            let open = world.open_facts_with_prefix("")?;
            match open.iter().find(|f| f.entity.eq_ignore_ascii_case(name)) {
                Some(f) => forget(world, &f.entity.clone(), session, now_ms),
                None => Ok(None),
            }
        }
    }
}

enum Found<'a> {
    None,
    One(&'a Fact),
    Many(Vec<&'a Fact>),
}

const STOP: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "that", "this", "it", "its", "of",
    "in", "on", "at", "to", "my", "our", "your", "you", "has", "have", "there", "so", "still",
    "now", "and", "or", "not",
];

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !STOP.contains(w))
        .map(str::to_string)
        .collect()
}

/// "lock" matches "locked", "temp" matches "temperature".
fn same_word(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short == long || (short.len() >= 4 && long.starts_with(short))
}

/// The open fact `subject` names. An exact entity wins; otherwise the facts
/// whose entity shares the most words with it (its value's words count too,
/// but a value alone never picks a fact: "locked" names no particular lock).
fn find<'a>(open: &'a [Fact], subject: &str) -> Found<'a> {
    let subject = subject.trim();
    if let Some(f) = open.iter().find(|f| f.entity.eq_ignore_ascii_case(subject)) {
        return Found::One(f);
    }
    let want = words(subject);
    let mut best = 0usize;
    let mut hits: Vec<&Fact> = Vec::new();
    for f in open {
        let entity = words(&f.entity);
        let on_entity = want
            .iter()
            .filter(|w| entity.iter().any(|e| same_word(w, e)))
            .count();
        if on_entity == 0 {
            continue;
        }
        let value = words(&show(&f.value));
        let on_value = want
            .iter()
            .filter(|w| !entity.iter().any(|e| same_word(w, e)))
            .filter(|w| value.iter().any(|v| same_word(w, v)))
            .count();
        let score = on_entity + on_value;
        if score > best {
            best = score;
            hits.clear();
        }
        if score == best {
            hits.push(f);
        }
    }
    match hits.len() {
        0 => Found::None,
        1 => Found::One(hits[0]),
        _ => Found::Many(hits.into_iter().take(5).collect()),
    }
}

fn show(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() > 120 {
        let cut: String = s.chars().take(119).collect();
        format!("{cut}…")
    } else {
        s
    }
}

fn when(ms: u64, now_ms: u64) -> String {
    let at = chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("{ms} ms"));
    format!("{at}, {}", crate::world_context::age(now_ms, ms))
}

fn origin_words(f: &Fact) -> String {
    match f.origin {
        Origin::Observed => format!("a reading: `{}` reported it", f.source),
        Origin::Derived => format!("computed by `{}`", f.source),
        Origin::Asserted => format!("an agent's conclusion, recorded by `{}`", f.source),
        Origin::Instructed => format!("a person said so, through `{}`", f.source),
    }
}

fn one_fact(f: &Fact) -> String {
    format!("#{} {} = {}", f.id, f.entity, show(&f.value))
}

fn explain(world: &WorldMemory, f: &Fact, now_ms: u64) -> Result<String> {
    let mut t = format!(
        "I believe {} = {} (fact #{}).\nBecause: {}, at {}.\n",
        f.entity,
        show(&f.value),
        f.id,
        origin_words(f),
        when(f.valid_from, now_ms)
    );
    let support = world.support_status(f)?;
    t.push_str(&match support {
        Support::Unknown => "Rests on: not recorded.\n".to_string(),
        Support::SelfStanding => "Rests on: nothing else; it stands on its own.\n".to_string(),
        Support::Grounded | Support::Ungrounded { .. } => {
            let ids: Vec<i64> = f.derived_from.iter().flatten().flatten().copied().collect();
            let mut parts = Vec::new();
            for id in ids {
                parts.push(match world.fact_by_id(id)? {
                    Some(s) if s.valid_to.is_none() => one_fact(&s),
                    Some(s) => format!("{} (no longer believed)", one_fact(&s)),
                    None => format!("#{id} (gone)"),
                });
            }
            let head = if support.has_failed() {
                "Rests on, and has lost its grounds"
            } else {
                "Rests on"
            };
            format!("{head}: {}.\n", parts.join("; "))
        }
    });
    let deps: Vec<Fact> = world
        .dependents(f.id)?
        .into_iter()
        .filter(|d| d.valid_to.is_none())
        .collect();
    if !deps.is_empty() {
        let shown: Vec<String> = deps.iter().take(5).map(one_fact).collect();
        let more = deps.len().saturating_sub(5);
        t.push_str(&format!("Depends on it: {}", shown.join("; ")));
        if more > 0 {
            t.push_str(&format!(" and {more} more"));
        }
        t.push_str(".\n");
    }
    t.push_str("Say \"forget that\" to withdraw it.");
    Ok(t)
}

fn forget(world: &WorldMemory, entity: &str, session: &str, now_ms: u64) -> Result<Option<Answer>> {
    let Some(was) = world.current(entity)? else {
        return Ok(Some(Answer {
            text: format!("I no longer hold any belief about {entity}."),
            referent: None,
        }));
    };
    let why = Closure::Operator(format!("forgotten on request in chat ({session})"));
    let closed = world.withdraw_entity(entity, now_ms, &why)?;
    let mut t = format!(
        "Forgotten: {entity} (was {}, fact #{}, from `{}`). It stays in history as withdrawn; \
         a new report from `{}` will be believed again.",
        show(&was.value),
        was.id,
        was.source,
        was.source
    );
    let mut lost = Vec::new();
    for id in &closed {
        for d in world.dependents(*id)? {
            if d.valid_to.is_none() && world.support_status(&d)?.has_failed() {
                lost.push(one_fact(&d));
            }
        }
    }
    if !lost.is_empty() {
        t.push_str(&format!(
            "\nThese rested on it and have lost their grounds: {}.",
            lost.join("; ")
        ));
    }
    Ok(Some(Answer {
        text: t,
        referent: None,
    }))
}

impl Agent {
    /// Answer a belief command from world memory, or `None` to run the turn.
    /// Any message that is not one ends what "forget that" would refer to.
    pub(crate) fn belief_command(
        &self,
        session_id: &str,
        user_message: &str,
    ) -> Result<Option<AgentResponse>> {
        let Some(world) = &self.world else {
            return Ok(None);
        };
        let referent = {
            let mut map = self.belief_referents.lock().unwrap();
            map.remove(session_id)
        };
        let Some(cmd) = parse(user_message) else {
            return Ok(None);
        };
        let now_ms = crate::now_ms();
        let Some(a) = answer(world, &cmd, referent.as_deref(), session_id, now_ms)? else {
            return Ok(None);
        };
        if let Some(r) = &a.referent {
            self.belief_referents
                .lock()
                .unwrap()
                .insert(session_id.to_string(), r.clone());
        }
        self.memory.create_session_with_id(session_id)?;
        self.memory
            .append_message(session_id, ChatRole::User, user_message)?;
        self.memory
            .append_message(session_id, ChatRole::Assistant, &a.text)?;
        tracing::info!(session = %session_id, command = ?cmd, "belief command answered from world memory");
        Ok(Some(AgentResponse {
            message: a.text,
            provider: "world-memory".to_string(),
            ..Default::default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commands_parse_and_everything_else_does_not() {
        assert_eq!(
            parse("Why do you believe the front door is locked?"),
            Some(Command::Why("the front door is locked".into()))
        );
        assert_eq!(
            parse("why do you think that the greenhouse is hot"),
            Some(Command::Why("the greenhouse is hot".into()))
        );
        assert_eq!(parse("Forget that."), Some(Command::ForgetThat));
        assert_eq!(
            parse("forget greenhouse.temp"),
            Some(Command::Forget("greenhouse.temp".into()))
        );
        assert_eq!(parse("why do you believe that?"), None);
        assert_eq!(parse("what do you believe about the door"), None);
        assert_eq!(parse("don't forget the milk"), None);
    }

    #[test]
    fn a_lookup_names_the_source_the_support_and_what_depends_on_it() {
        let w = WorldMemory::open_in_memory().unwrap();
        let t = 1_700_000_000_000;
        let r = w
            .observe_as(
                "front_door.lock",
                json!("locked"),
                t,
                t,
                "node-3",
                Origin::Observed,
            )
            .unwrap();
        w.observe_as(
            "garage.light",
            json!("off"),
            t,
            t,
            "node-4",
            Origin::Observed,
        )
        .unwrap();
        w.observe_as_derived_from(
            "house.secure",
            json!(true),
            t,
            t,
            "rollup",
            Origin::Derived,
            &[r.id],
        )
        .unwrap();
        let a = answer(
            &w,
            &Command::Why("the front door is locked".into()),
            None,
            "tg-1",
            t + 60_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.referent.as_deref(), Some("front_door.lock"));
        assert!(
            a.text.contains("I believe front_door.lock = locked"),
            "{}",
            a.text
        );
        assert!(
            a.text.contains("a reading: `node-3` reported it"),
            "{}",
            a.text
        );
        assert!(
            a.text.contains("2023-11-14 22:13 UTC, 60s ago"),
            "{}",
            a.text
        );
        assert!(
            a.text.contains("Depends on it: #3 house.secure = true"),
            "{}",
            a.text
        );
        // Nothing about the weather is held: the model gets the question.
        assert!(
            answer(&w, &Command::Why("it will rain".into()), None, "tg-1", t)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn forgetting_withdraws_by_operator_closure_and_names_what_lost_its_grounds() {
        let w = WorldMemory::open_in_memory().unwrap();
        let t = 1_700_000_000_000;
        let r = w
            .observe_as(
                "front_door.lock",
                json!("locked"),
                t,
                t,
                "node-3",
                Origin::Observed,
            )
            .unwrap();
        w.observe_as_derived_from(
            "house.secure",
            json!(true),
            t,
            t,
            "rollup",
            Origin::Derived,
            &[r.id],
        )
        .unwrap();
        // "forget that" with nothing looked up is not a command.
        assert!(answer(&w, &Command::ForgetThat, None, "tg-1", t)
            .unwrap()
            .is_none());
        // "forget" of something not held is not either.
        assert!(
            answer(&w, &Command::Forget("the milk".into()), None, "tg-1", t)
                .unwrap()
                .is_none()
        );
        assert!(w.current("front_door.lock").unwrap().is_some());

        let a = answer(
            &w,
            &Command::ForgetThat,
            Some("front_door.lock"),
            "tg-1",
            t + 1,
        )
        .unwrap()
        .unwrap();
        assert!(
            a.text.starts_with("Forgotten: front_door.lock (was locked"),
            "{}",
            a.text
        );
        assert!(
            a.text
                .contains("lost their grounds: #2 house.secure = true"),
            "{}",
            a.text
        );
        assert!(w.current("front_door.lock").unwrap().is_none());
        let h = w.history("front_door.lock").unwrap();
        assert_eq!(
            Closure::of(&h[0]),
            Closure::Operator("forgotten on request in chat (tg-1)".into())
        );
    }
}
