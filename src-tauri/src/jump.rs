//! The panel's jump line: today, or the last few days, running well above a usual
//! day. It reads the gauge's own day totals; nothing is stored for it.
//!
//! A usual day is the average of the days with any spend among the 7 complete days
//! before. Days with nothing spent are left out, so a weekend or a day off does
//! not make the next working day look like a jump. With fewer than 3 such days
//! there is no usual day to compare with, and no line.
//!
//! The jump never touches the ring: the ring is today against the allowance, and a
//! jump from $5 to $15 on a $50 allowance is not a budget problem.

use crate::budget::first_of_month;
use chrono::{Duration, NaiveDate};
use serde::Serialize;
use std::collections::BTreeMap;

/// How many times a usual day counts as a jump.
pub const TIMES: f64 = 1.5;
/// How far above a usual day a jump must also be, as a share of the daily allowance.
/// A part day is only compared once it is already big, so a quiet morning never
/// reads as anything.
pub const ALLOWANCE_SHARE: f64 = 0.25;
/// That margin in dollars when no budget is set, so cents do not make a jump.
pub const NO_BUDGET_MARGIN: f64 = 1.0;
/// Below this a day counts as a day with nothing spent.
pub const ACTIVE: f64 = 0.01;
/// The fewest days with spend a usual day is worked out from.
pub const MIN_ACTIVE: usize = 3;
pub const BASELINE_DAYS: i64 = 7;
pub const RECENT_DAYS: i64 = 3;
/// How far back the gauge keeps calls before today: the recent days and the week before them.
pub const HISTORY_DAYS: i64 = RECENT_DAYS + BASELINE_DAYS;
/// "Mostly from" a project: its share of the rise must be more than this.
pub const MOSTLY: f64 = 0.5;

/// One local day's spend, in all and by project folder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DaySpend {
    pub total: f64,
    pub by_project: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JumpKind {
    /// Today so far, against a usual day.
    Today,
    /// The last 3 complete days on average, against the week before them.
    Recent,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Jump {
    pub kind: JumpKind,
    /// `spent` over `usual`.
    pub times: f64,
    /// Today so far, or the recent days' average.
    pub spent: f64,
    pub usual: f64,
    /// The project folder behind most of the rise, when one is.
    pub project: Option<String>,
}

/// The first day the gauge keeps calls for: the 1st of the month, or earlier in
/// the first days of a month, so the jump has its days to compare.
pub fn history_start(today: NaiveDate) -> NaiveDate {
    first_of_month(today).min(today - Duration::days(HISTORY_DAYS))
}

/// The active days from `from` to `to`, both included, and their average.
fn baseline(days: &BTreeMap<NaiveDate, DaySpend>, from: NaiveDate, to: NaiveDate) -> Option<(f64, Vec<&DaySpend>)> {
    let active: Vec<&DaySpend> = days.range(from..=to).map(|(_, s)| s).filter(|s| s.total >= ACTIVE).collect();
    if active.len() < MIN_ACTIVE {
        return None;
    }
    let usual = active.iter().map(|s| s.total).sum::<f64>() / active.len() as f64;
    Some((usual, active))
}

fn margin(daily_allowance: f64) -> f64 {
    if daily_allowance > 0.0 { daily_allowance * ALLOWANCE_SHARE } else { NO_BUDGET_MARGIN }
}

fn is_jump(spent: f64, usual: f64, daily_allowance: f64) -> bool {
    spent >= TIMES * usual && spent - usual >= margin(daily_allowance)
}

/// The project whose average grew most from `before` to `now`, when that is more
/// than half of the whole rise.
fn mostly(now: &[&DaySpend], before: &[&DaySpend]) -> Option<String> {
    let avg = |set: &[&DaySpend], p: &str| set.iter().map(|s| s.by_project.get(p).copied().unwrap_or(0.0)).sum::<f64>() / set.len() as f64;
    let total = |set: &[&DaySpend]| set.iter().map(|s| s.total).sum::<f64>() / set.len() as f64;
    let rise = total(now) - total(before);
    if rise <= 0.0 {
        return None;
    }
    let mut names: Vec<&String> = now.iter().flat_map(|s| s.by_project.keys()).collect();
    names.sort();
    names.dedup();
    let (name, grew) = names
        .into_iter()
        .map(|p| (p, avg(now, p) - avg(before, p)))
        // The largest rise; a tie goes to the first name.
        .fold(None::<(&String, f64)>, |best, (p, g)| match best {
            Some((_, b)) if b >= g => best,
            _ => Some((p, g)),
        })?;
    (grew > MOSTLY * rise).then(|| name.clone())
}

/// The jump to show, if any. `days` holds the gauge's day totals up to and
/// including `today`; a day missing from it spent nothing. Today is checked
/// first; the recent days only when today is not a jump.
pub fn jump(today: NaiveDate, days: &BTreeMap<NaiveDate, DaySpend>, daily_allowance: f64) -> Option<Jump> {
    let none = DaySpend::default();
    let day = |n: i64| days.get(&(today - Duration::days(n))).unwrap_or(&none);

    if let Some((usual, before)) = baseline(days, today - Duration::days(BASELINE_DAYS), today - Duration::days(1)) {
        let now = day(0);
        if is_jump(now.total, usual, daily_allowance) {
            return Some(Jump { kind: JumpKind::Today, times: now.total / usual, spent: now.total, usual, project: mostly(&[now], &before) });
        }
    }

    let (usual, before) = baseline(days, today - Duration::days(HISTORY_DAYS), today - Duration::days(RECENT_DAYS + 1))?;
    let recent: Vec<&DaySpend> = (1..=RECENT_DAYS).map(day).collect();
    let spent = recent.iter().map(|s| s.total).sum::<f64>() / RECENT_DAYS as f64;
    is_jump(spent, usual, daily_allowance)
        .then(|| Jump { kind: JumpKind::Recent, times: spent / usual, spent, usual, project: mostly(&recent, &before) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    /// Day totals ending today: `back[0]` is today, `back[1]` yesterday, and so on.
    /// Each day's spend is all in project "a".
    fn days(today: NaiveDate, back: &[f64]) -> BTreeMap<NaiveDate, DaySpend> {
        back.iter()
            .enumerate()
            .filter(|(_, v)| **v > 0.0)
            .map(|(i, v)| (today - Duration::days(i as i64), DaySpend { total: *v, by_project: [("a".to_string(), *v)].into() }))
            .collect()
    }

    const TODAY: (i32, u32, u32) = (2026, 11, 18);
    fn today() -> NaiveDate {
        d(TODAY.0, TODAY.1, TODAY.2)
    }

    #[test]
    fn today_at_1_5_times_a_usual_day_and_a_quarter_allowance_above_it_is_a_jump() {
        // A usual day of $40; a $50 allowance, so the margin is $12.50.
        let j = jump(today(), &days(today(), &[84.0, 40.0, 40.0, 40.0, 40.0, 40.0]), 50.0).unwrap();
        assert_eq!(j.kind, JumpKind::Today);
        assert!((j.times - 2.1).abs() < 1e-9);
        assert_eq!((j.spent, j.usual), (84.0, 40.0));
    }

    #[test]
    fn both_thresholds_must_hold() {
        let base = [40.0, 40.0, 40.0];
        let at = |now: f64, allowance: f64| {
            let mut back = vec![now];
            back.extend(base);
            jump(today(), &days(today(), &back), allowance)
        };
        assert!(at(60.0, 50.0).is_some(), "1.5 times and $20 above, margin $12.50");
        assert!(at(59.99, 50.0).is_none(), "just under 1.5 times");
        // 1.5 times but only $20 above a $100 allowance's $25 margin.
        assert!(at(60.0, 100.0).is_none());
        assert!(at(65.0, 100.0).is_some(), "$25 above, at the margin");
    }

    #[test]
    fn a_small_usual_day_needs_the_dollar_margin_too() {
        // $0.40 a day to $1.00 is 2.5 times, but only $0.60 above.
        let small = days(today(), &[1.0, 0.4, 0.4, 0.4]);
        assert!(jump(today(), &small, 0.0).is_none(), "no budget: a $1 margin");
        assert!(jump(today(), &small, 2.0).is_some(), "a $2 allowance: a $0.50 margin");
        assert!(jump(today(), &days(today(), &[1.5, 0.4, 0.4, 0.4]), 0.0).is_some());
    }

    #[test]
    fn days_with_nothing_spent_are_left_out_of_a_usual_day() {
        // Monday after a weekend: 3 working days of $40 and two empty days.
        let j = jump(today(), &days(today(), &[50.0, 0.0, 0.0, 40.0, 40.0, 40.0]), 50.0);
        assert!(j.is_none(), "$50 is not a jump on a usual $40 day");
        let cents = days(today(), &[50.0, 0.005, 0.005, 40.0, 40.0, 40.0]);
        assert!(jump(today(), &cents, 50.0).is_none(), "half a cent is a day with nothing spent");
    }

    #[test]
    fn fewer_than_3_active_days_means_no_line() {
        assert!(jump(today(), &days(today(), &[500.0, 10.0, 10.0]), 50.0).is_none());
        assert!(jump(today(), &days(today(), &[500.0, 10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 10.0]), 50.0).is_none());
        assert!(jump(today(), &days(today(), &[500.0]), 50.0).is_none(), "a new install");
        assert!(jump(today(), &BTreeMap::new(), 50.0).is_none());
        assert!(jump(today(), &days(today(), &[500.0, 10.0, 10.0, 10.0]), 50.0).is_some(), "3 is enough");
    }

    #[test]
    fn the_usual_day_is_the_previous_7_complete_days_only() {
        // An 8th day back of $1000 is outside the week.
        let mut back = vec![84.0, 40.0, 40.0, 40.0, 0.0, 0.0, 0.0, 0.0, 1000.0];
        assert!(jump(today(), &days(today(), &back), 50.0).is_some());
        back[7] = 1000.0;
        assert!(jump(today(), &days(today(), &back), 50.0).is_none(), "the 7th day back is inside it");
    }

    #[test]
    fn three_days_up_on_the_week_before_is_a_recent_jump() {
        // Today is quiet; the 3 days before it average $72 against $40.
        let back = [5.0, 72.0, 72.0, 72.0, 40.0, 40.0, 40.0, 40.0, 40.0, 0.0, 0.0];
        let j = jump(today(), &days(today(), &back), 50.0).unwrap();
        assert_eq!(j.kind, JumpKind::Recent);
        assert!((j.times - 1.8).abs() < 1e-9);
        assert_eq!((j.spent, j.usual), (72.0, 40.0));
    }

    #[test]
    fn a_recent_jump_needs_its_own_week_of_3_active_days() {
        let back = [5.0, 72.0, 72.0, 72.0, 40.0, 40.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert!(jump(today(), &days(today(), &back), 50.0).is_none(), "2 active days");
        // The 11th day back is outside its week; the 10th is inside it.
        let mut back = vec![5.0, 72.0, 72.0, 72.0, 40.0, 40.0, 0.0, 0.0, 0.0, 0.0, 0.0, 40.0];
        assert!(jump(today(), &days(today(), &back), 50.0).is_none());
        back[10] = 40.0;
        assert!(jump(today(), &days(today(), &back), 50.0).is_some());
    }

    #[test]
    fn one_heavy_day_in_three_is_not_a_step_up() {
        // $120, $20, $20 average $53, under 1.5 times $40.
        let back = [5.0, 120.0, 20.0, 20.0, 40.0, 40.0, 40.0, 40.0];
        assert!(jump(today(), &days(today(), &back), 50.0).is_none());
    }

    #[test]
    fn today_wins_over_the_recent_days() {
        let back = [200.0, 72.0, 72.0, 72.0, 40.0, 40.0, 40.0, 40.0, 40.0];
        assert_eq!(jump(today(), &days(today(), &back), 50.0).unwrap().kind, JumpKind::Today);
    }

    fn with_projects(entries: &[(i64, &[(&str, f64)])]) -> BTreeMap<NaiveDate, DaySpend> {
        entries
            .iter()
            .map(|(n, ps)| {
                let by_project: BTreeMap<String, f64> = ps.iter().map(|(p, v)| (p.to_string(), *v)).collect();
                (today() - Duration::days(*n), DaySpend { total: by_project.values().sum(), by_project })
            })
            .collect()
    }

    #[test]
    fn it_names_the_project_behind_most_of_the_rise() {
        let base: &[(&str, f64)] = &[("app", 40.0)];
        let all = with_projects(&[(0, &[("app", 44.0), ("new-thing", 40.0)]), (1, base), (2, base), (3, base)]);
        assert_eq!(jump(today(), &all, 50.0).unwrap().project.as_deref(), Some("new-thing"));
    }

    #[test]
    fn no_project_is_named_when_the_rise_is_spread_out() {
        let base: &[(&str, f64)] = &[("a", 20.0), ("b", 20.0)];
        let all = with_projects(&[(0, &[("a", 42.0), ("b", 42.0)]), (1, base), (2, base), (3, base)]);
        let j = jump(today(), &all, 50.0).unwrap();
        assert_eq!(j.project, None, "each is exactly half of the rise: named only when one is more");
        let all = with_projects(&[(0, &[("a", 50.0), ("b", 34.0)]), (1, base), (2, base), (3, base)]);
        assert_eq!(jump(today(), &all, 50.0).unwrap().project.as_deref(), Some("a"));
    }

    #[test]
    fn a_recent_jump_names_its_project_over_the_3_days() {
        let base: &[(&str, f64)] = &[("app", 40.0)];
        let up: &[(&str, f64)] = &[("app", 40.0), ("new-thing", 32.0)];
        let all = with_projects(&[(1, up), (2, up), (3, up), (4, base), (5, base), (6, base)]);
        let j = jump(today(), &all, 50.0).unwrap();
        assert_eq!((j.kind, j.project.as_deref()), (JumpKind::Recent, Some("new-thing")));
    }

    #[test]
    fn history_reaches_10_days_back_in_the_first_days_of_a_month() {
        assert_eq!(history_start(d(2026, 11, 18)), d(2026, 11, 1));
        assert_eq!(history_start(d(2026, 11, 11)), d(2026, 11, 1));
        assert_eq!(history_start(d(2026, 11, 10)), d(2026, 10, 31));
        assert_eq!(history_start(d(2026, 11, 1)), d(2026, 10, 22));
        assert_eq!(history_start(d(2027, 1, 3)), d(2026, 12, 24), "across a year end");
        assert_eq!(history_start(d(2028, 3, 2)), d(2028, 2, 21), "across a leap day");
    }
}
