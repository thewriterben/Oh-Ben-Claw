//! Natural-language schedules — "every weekday at 8", "in 20 minutes",
//! "tomorrow at 9:30", "every monday and thursday at 7pm" — turned into a
//! [`TaskKind`] deterministically (parity plan Stage 2, item 6, 2026-09-11).
//!
//! The model is the LLM step: it reads the operator's sentence and calls the
//! `schedule` tool with the *when* part. This module then does the conversion
//! with no second model call, so the same phrase always yields the same
//! schedule and the result can be tested. A phrase this grammar does not cover
//! comes back as an error that lists the forms it does, and the tool also
//! accepts a 6-field cron expression for anything else.
//!
//! Times are read in the scheduler's configured zone ([`Tz`]); a bare hour
//! ("at 8") is 08:00, "8pm" is 20:00, "noon"/"midnight" work.

use crate::{TaskKind, Tz};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};

/// What a phrase parsed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub kind: TaskKind,
    /// A short human rendering of the schedule, e.g. `weekdays at 08:00`.
    pub description: String,
}

/// The forms `parse_when` understands, for error messages and tool descriptions.
pub const FORMS: &str = "every N seconds|minutes|hours; every minute|hour|half hour; \
every day|weekday|weekend at TIME; every monday[, wednesday...] at TIME; \
every month on the Nth at TIME; in N minutes|hours|days; \
[today|tomorrow|on monday] at TIME; TIME like 8, 8am, 8:30pm, 17:45, noon, midnight";

/// Parse a schedule phrase relative to `now_ts` (Unix seconds) in zone `tz`.
pub fn parse_when(phrase: &str, now_ts: u64, tz: Tz) -> Result<Parsed, String> {
    let text = normalise(phrase);
    if text.is_empty() {
        return Err(format!("empty schedule; say when, e.g. {FORMS}"));
    }
    let words: Vec<&str> = text.split_whitespace().collect();

    if let Some(p) = parse_interval(&words)? {
        return Ok(p);
    }
    if let Some(p) = parse_recurring(&words)? {
        return Ok(p);
    }
    if let Some(p) = parse_one_shot(&words, now_ts, tz)? {
        return Ok(p);
    }
    Err(format!(
        "could not read \"{}\" as a schedule; accepted forms: {FORMS}",
        phrase.trim()
    ))
}

/// Human rendering of a stored kind, for listings.
pub fn describe(kind: &TaskKind, tz: Tz) -> String {
    match kind {
        TaskKind::Interval(secs) => format!("every {}", human_secs(*secs)),
        TaskKind::Cron(expr) => format!("cron `{expr}` ({})", tz.as_str()),
        TaskKind::OneShot(ts) => format!("once at {}", tz.render(*ts)),
    }
}

// ── pieces ────────────────────────────────────────────────────────────────────

fn normalise(s: &str) -> String {
    let lowered = s.trim().to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    for ch in lowered.chars() {
        match ch {
            ',' => out.push(' '),
            '.' if !out.ends_with(|c: char| c.is_ascii_digit()) => out.push(' '),
            c if c.is_alphanumeric() || c == ':' || c == '.' || c == ' ' => out.push(c),
            _ => out.push(' '),
        }
    }
    // "8 am" → "8am", "at 5 pm" → "at 5pm"; "and" is a separator we drop.
    let mut words: Vec<String> = Vec::new();
    for w in out.split_whitespace() {
        if matches!(w, "am" | "pm" | "a.m" | "p.m")
            && words
                .last()
                .is_some_and(|p| p.chars().next().is_some_and(|c| c.is_ascii_digit()))
        {
            let last = words.last_mut().unwrap();
            last.push_str(&w[..1]);
            last.push('m');
            continue;
        }
        if matches!(w, "and" | "the" | "each" | "please" | "o'clock" | "oclock") {
            continue;
        }
        words.push(w.to_string());
    }
    words.join(" ")
}

fn unit_secs(word: &str) -> Option<u64> {
    Some(match word {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86_400,
        "w" | "week" | "weeks" => 7 * 86_400,
        _ => return None,
    })
}

fn number(word: &str) -> Option<u64> {
    if let Ok(n) = word.parse::<u64>() {
        return Some(n);
    }
    Some(match word {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "fifteen" => 15,
        "twenty" => 20,
        "thirty" => 30,
        "forty" => 40,
        "fifty" => 50,
        "sixty" => 60,
        "ninety" => 90,
        _ => return None,
    })
}

fn human_secs(secs: u64) -> String {
    if secs.is_multiple_of(86_400) && secs >= 86_400 {
        let d = secs / 86_400;
        return if d == 1 {
            "day".into()
        } else {
            format!("{d} days")
        };
    }
    if secs.is_multiple_of(3600) && secs >= 3600 {
        let h = secs / 3600;
        return if h == 1 {
            "hour".into()
        } else {
            format!("{h} hours")
        };
    }
    if secs.is_multiple_of(60) && secs >= 60 {
        let m = secs / 60;
        return if m == 1 {
            "minute".into()
        } else {
            format!("{m} minutes")
        };
    }
    format!("{secs} seconds")
}

/// `every N units`, `every unit`, `every half hour`, `hourly`, `every other minute`.
fn parse_interval(words: &[&str]) -> Result<Option<Parsed>, String> {
    let secs = match words {
        ["hourly"] => 3600,
        ["every", "half", "hour"] | ["every", "half", "an", "hour"] => 1800,
        ["every", "other", unit] => 2 * unit_secs(unit).ok_or_else(|| bad_unit(unit))?,
        ["every", unit] if unit_secs(unit).is_some() => {
            let s = unit_secs(unit).unwrap();
            if s >= 86_400 {
                return Err("say when in the day, e.g. 'every day at 8'".into());
            }
            s
        }
        ["every", n, unit] if number(n).is_some() && unit_secs(unit).is_some() => {
            let s = number(n).unwrap() * unit_secs(unit).unwrap();
            if s >= 86_400 {
                return Err("for daily or longer, say the time: 'every day at 8'".into());
            }
            s
        }
        _ => return Ok(None),
    };
    if secs < 5 {
        return Err("the shortest interval is 5 seconds".into());
    }
    Ok(Some(Parsed {
        kind: TaskKind::Interval(secs),
        description: format!("every {}", human_secs(secs)),
    }))
}

fn bad_unit(u: &str) -> String {
    format!("unknown unit '{u}' (seconds, minutes, hours)")
}

/// `HH`, `HHam`, `HH:MM`, `HH:MMpm`, `noon`, `midnight`.
fn parse_time(word: &str) -> Option<NaiveTime> {
    match word {
        "noon" | "midday" => return NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    let (body, meridiem) = if let Some(b) = word.strip_suffix("am") {
        (b, Some(false))
    } else if let Some(b) = word.strip_suffix("pm") {
        (b, Some(true))
    } else {
        (word, None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    let h = match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            match (h, pm) {
                (12, false) => 0,
                (12, true) => 12,
                (h, true) => h + 12,
                (h, false) => h,
            }
        }
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

fn weekday_token(word: &str) -> Option<(&'static str, chrono::Weekday)> {
    use chrono::Weekday::*;
    Some(match word.trim_end_matches('s') {
        "mon" | "monday" => ("Mon", Mon),
        "tue" | "tues" | "tuesday" => ("Tue", Tue),
        "wed" | "wednesday" => ("Wed", Wed),
        "thu" | "thur" | "thurs" | "thursday" => ("Thu", Thu),
        "fri" | "friday" => ("Fri", Fri),
        "sat" | "saturday" => ("Sat", Sat),
        "sun" | "sunday" => ("Sun", Sun),
        _ => return None,
    })
}

/// Split `[... at TIME]` off the end; returns (head, time).
fn split_at_time<'a>(words: &'a [&'a str]) -> Result<(&'a [&'a str], Option<NaiveTime>), String> {
    if let Some(pos) = words.iter().rposition(|w| *w == "at") {
        let rest = &words[pos + 1..];
        let t = match rest {
            [t] => parse_time(t),
            [t, "in", "morning"] => parse_time(t),
            [t, "in", "afternoon"] | [t, "in", "evening"] | [t, "at", "night"] => parse_time(t)
                .map(|t| {
                    if t.hour() < 12 {
                        t + Duration::hours(12)
                    } else {
                        t
                    }
                }),
            _ => None,
        };
        let t = t.ok_or_else(|| format!("could not read the time '{}'", rest.join(" ")))?;
        return Ok((&words[..pos], Some(t)));
    }
    Ok((words, None))
}

/// `every day|weekday|weekend|<days> at TIME`, `daily at TIME`, `weekdays at TIME`,
/// `every month on Nth at TIME`.
fn parse_recurring(words: &[&str]) -> Result<Option<Parsed>, String> {
    let (head, time) = split_at_time(words)?;
    let head: Vec<&str> = head
        .iter()
        .copied()
        .filter(|w| *w != "every" && *w != "on")
        .collect();
    let is_every = words.first().is_some_and(|w| *w == "every")
        || matches!(
            head.first(),
            Some(&"daily") | Some(&"weekdays") | Some(&"weekends") | Some(&"monthly")
        );
    if !is_every {
        return Ok(None);
    }
    let need_time = |what: &str| -> Result<NaiveTime, String> {
        time.ok_or_else(|| format!("say the time for '{what}', e.g. '{what} at 8'"))
    };
    // Monthly: `month Nth`, `monthly Nth`.
    if matches!(head.first(), Some(&"month") | Some(&"monthly")) {
        let day = head
            .get(1)
            .and_then(|d| {
                d.trim_end_matches(['s', 't', 'n', 'd', 'r', 'h'])
                    .parse::<u32>()
                    .ok()
            })
            .filter(|d| (1..=31).contains(d))
            .ok_or_else(|| {
                "say the day of the month, e.g. 'every month on the 1st at 9'".to_string()
            })?;
        let t = need_time("every month on the 1st")?;
        return Ok(Some(Parsed {
            kind: TaskKind::Cron(format!("0 {} {} {} * *", t.minute(), t.hour(), day)),
            description: format!("monthly on the {} at {}", ordinal(day), hm(t)),
        }));
    }
    let dow: Option<(String, String)> = match head.as_slice() {
        ["day"] | ["daily"] => Some(("*".into(), "daily".into())),
        ["weekday"] | ["weekdays"] => Some(("Mon-Fri".into(), "weekdays".into())),
        ["weekend"] | ["weekends"] => Some(("Sat,Sun".into(), "weekends".into())),
        days if !days.is_empty() && days.iter().all(|d| weekday_token(d).is_some()) => {
            let toks: Vec<&str> = days.iter().map(|d| weekday_token(d).unwrap().0).collect();
            Some((toks.join(","), format!("every {}", toks.join(", "))))
        }
        _ => None,
    };
    let Some((dow_field, label)) = dow else {
        return Ok(None);
    };
    let t = need_time(&format!("every {}", head.join(" ")))?;
    Ok(Some(Parsed {
        kind: TaskKind::Cron(format!("0 {} {} * * {}", t.minute(), t.hour(), dow_field)),
        description: format!("{label} at {}", hm(t)),
    }))
}

/// `in N units`, `at TIME`, `today|tonight|tomorrow at TIME`, `on <day> at TIME`,
/// `next <day> at TIME`.
fn parse_one_shot(words: &[&str], now_ts: u64, tz: Tz) -> Result<Option<Parsed>, String> {
    if let ["in", n, unit] = words {
        let n = number(n).ok_or_else(|| format!("could not read '{n}' as a number"))?;
        let secs = unit_secs(unit).ok_or_else(|| bad_unit(unit))?;
        if n * secs < 5 {
            return Err("the soonest one-shot is 5 seconds from now".into());
        }
        let ts = now_ts + n * secs;
        let span = human_secs(n * secs);
        // human_secs says "minute" for 60 (it reads well after "every"); after
        // "in" it needs the one.
        let span = if span.starts_with(|c: char| c.is_ascii_digit()) {
            span
        } else {
            format!("1 {span}")
        };
        return Ok(Some(Parsed {
            kind: TaskKind::OneShot(ts),
            description: format!("once at {} (in {span})", tz.render(ts)),
        }));
    }
    let (head, time) = split_at_time(words)?;
    let Some(t) = time else {
        return Ok(None);
    };
    let now = tz.naive(now_ts).ok_or("clock out of range")?;
    let today = now.date();
    let head: Vec<&str> = head.iter().copied().filter(|w| *w != "on").collect();
    let date: NaiveDate = match head.as_slice() {
        [] | ["today"] | ["tonight"] => {
            if today.and_time(t) > now {
                today
            } else {
                today.succ_opt().ok_or("date out of range")?
            }
        }
        ["tomorrow"] => today.succ_opt().ok_or("date out of range")?,
        [d] | ["next", d] if weekday_token(d).is_some() => {
            let want = weekday_token(d).unwrap().1;
            let mut cand = today;
            loop {
                if cand.weekday() == want && cand.and_time(t) > now {
                    break cand;
                }
                cand = cand.succ_opt().ok_or("date out of range")?;
            }
        }
        _ => return Ok(None),
    };
    let ts = tz
        .to_ts(date.and_time(t))
        .ok_or("that local time does not exist (DST gap)")?;
    Ok(Some(Parsed {
        kind: TaskKind::OneShot(ts),
        description: format!("once at {}", tz.render(ts)),
    }))
}

fn hm(t: NaiveTime) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

fn ordinal(d: u32) -> String {
    let suffix = match (d % 10, d % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{d}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-11 15:00:00 UTC, a Friday.
    const NOW: u64 = 1_789_138_800;

    fn p(s: &str) -> Parsed {
        parse_when(s, NOW, Tz::Utc).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn intervals() {
        assert_eq!(p("every 5 minutes").kind, TaskKind::Interval(300));
        assert_eq!(p("every minute").kind, TaskKind::Interval(60));
        assert_eq!(p("Every 2 hours").kind, TaskKind::Interval(7200));
        assert_eq!(p("hourly").kind, TaskKind::Interval(3600));
        assert_eq!(p("every half hour").kind, TaskKind::Interval(1800));
        assert_eq!(p("every other minute").kind, TaskKind::Interval(120));
        assert_eq!(p("every ten seconds").description, "every 10 seconds");
        assert!(parse_when("every 2 seconds", NOW, Tz::Utc).is_err());
        assert!(parse_when("every day", NOW, Tz::Utc)
            .unwrap_err()
            .contains("every day at 8"));
    }

    #[test]
    fn recurring_cron_is_six_field() {
        assert_eq!(
            p("every weekday at 8").kind,
            TaskKind::Cron("0 0 8 * * Mon-Fri".into())
        );
        assert_eq!(p("every weekday at 8").description, "weekdays at 08:00");
        assert_eq!(
            p("every day at 7:30pm").kind,
            TaskKind::Cron("0 30 19 * * *".into())
        );
        assert_eq!(
            p("daily at noon").kind,
            TaskKind::Cron("0 0 12 * * *".into())
        );
        assert_eq!(
            p("every monday and thursday at 7 pm").kind,
            TaskKind::Cron("0 0 19 * * Mon,Thu".into())
        );
        assert_eq!(
            p("weekends at 10am").kind,
            TaskKind::Cron("0 0 10 * * Sat,Sun".into())
        );
        assert_eq!(
            p("every month on the 1st at 9").kind,
            TaskKind::Cron("0 0 9 1 * *".into())
        );
        assert_eq!(
            p("every month on the 1st at 9").description,
            "monthly on the 1st at 09:00"
        );
        assert_eq!(
            p("every Sunday at 12am").kind,
            TaskKind::Cron("0 0 0 * * Sun".into())
        );
        // every cron we emit must parse
        for s in [
            "every weekday at 8",
            "every day at 7:30pm",
            "every monday and thursday at 7 pm",
            "every month on the 15th at 17:45",
        ] {
            assert!(p(s).kind.validate().is_ok(), "{s}");
        }
        assert!(parse_when("every monday", NOW, Tz::Utc).is_err());
    }

    #[test]
    fn one_shots() {
        assert_eq!(p("in 20 minutes").kind, TaskKind::OneShot(NOW + 1200));
        assert_eq!(p("in an hour").kind, TaskKind::OneShot(NOW + 3600));
        assert!(p("in an hour").description.ends_with("(in 1 hour)"));
        assert!(p("in 20 minutes").description.ends_with("(in 20 minutes)"));
        assert_eq!(p("in 2 days").kind, TaskKind::OneShot(NOW + 2 * 86_400));
        // 15:00 UTC now: "at 16:00" is today, "at 8" is tomorrow
        assert_eq!(p("at 16:00").kind, TaskKind::OneShot(NOW + 3600));
        assert_eq!(p("at 8").kind, TaskKind::OneShot(NOW - 7 * 3600 + 86_400));
        assert_eq!(
            p("tomorrow at 9am").kind,
            TaskKind::OneShot(NOW - 6 * 3600 + 86_400)
        );
        assert_eq!(p("today at 5pm").kind, TaskKind::OneShot(NOW + 2 * 3600));
        // Friday 15:00 → next Monday 08:00 = +3 days -7 h
        assert_eq!(
            p("on monday at 8").kind,
            TaskKind::OneShot(NOW + 3 * 86_400 - 7 * 3600)
        );
        // Friday at 16:00 is still today; Friday at 8 is next week
        assert_eq!(p("friday at 16:00").kind, TaskKind::OneShot(NOW + 3600));
        assert_eq!(
            p("friday at 8").kind,
            TaskKind::OneShot(NOW + 7 * 86_400 - 7 * 3600)
        );
        assert!(parse_when("in 2 seconds", NOW, Tz::Utc).is_err());
    }

    #[test]
    fn rejects_gibberish_with_the_forms() {
        let e = parse_when("whenever you feel like it", NOW, Tz::Utc).unwrap_err();
        assert!(e.contains("accepted forms"), "{e}");
        let e = parse_when("every weekday at half past", NOW, Tz::Utc).unwrap_err();
        assert!(e.contains("could not read the time"), "{e}");
    }

    #[test]
    fn describe_renders() {
        assert_eq!(
            describe(&TaskKind::Interval(900), Tz::Utc),
            "every 15 minutes"
        );
        assert_eq!(
            describe(&TaskKind::Cron("0 0 8 * * Mon-Fri".into()), Tz::Utc),
            "cron `0 0 8 * * Mon-Fri` (utc)"
        );
        assert!(describe(&TaskKind::OneShot(NOW), Tz::Utc).starts_with("once at 2026-09-11 15:00"));
    }
}
