//! The menu bar gauge's budget arithmetic: a monthly budget spread evenly over the
//! calendar month, read against what this Mac's Claude Code transcripts cost.
//!
//! Every input is a local calendar date, never a duration, so a 23 or 25 hour
//! daylight saving day is still one day. Today counts as a day elapsed and as a
//! day left: it has started, and there is still time to spend in it.

use chrono::{Datelike, NaiveDate};
use serde::Serialize;

/// Which colour the ring shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Ring {
    /// No budget is set, so there is nothing to measure against.
    None,
    Green,
    Amber,
    Red,
}

/// The two thresholds, as fractions of today's allowance (0.8 = 80%).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub amber: f64,
    pub red: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { amber: 0.8, red: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BudgetView {
    pub monthly_budget: f64,
    pub days_in_month: u32,
    /// Including today.
    pub days_elapsed: u32,
    /// Including today.
    pub days_left: u32,
    /// The budget spread evenly over the month.
    pub daily_allowance: f64,
    pub today: f64,
    /// Today's spend as a share of today's allowance (1.0 = all of it).
    pub today_share: f64,
    pub month_to_date: f64,
    /// What an even pace would have spent by the end of today.
    pub on_pace: f64,
    /// Month to date over days elapsed, times the days in the month.
    pub projected: f64,
    /// What is left of the budget, spread over the days left. Never below zero.
    pub remaining_per_day: f64,
    pub ring: Ring,
}

pub fn days_in_month(d: NaiveDate) -> u32 {
    let (y, m) = (d.year(), d.month());
    let first_next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, m + 1, 1) };
    first_next.and_then(|n| n.pred_opt()).map(|last| last.day()).unwrap_or(30)
}

pub fn first_of_month(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

/// The ring for a share of today's allowance. A share at a threshold takes that
/// threshold's colour: 80% of the allowance is amber, 100% is red.
pub fn ring(share: f64, t: Thresholds) -> Ring {
    if share >= t.red {
        Ring::Red
    } else if share >= t.amber {
        Ring::Amber
    } else {
        Ring::Green
    }
}

/// The figures the panel shows. A budget of zero or less means "not set": the
/// money figures are still reported, and the ring is `Ring::None`.
pub fn view(monthly_budget: f64, today: NaiveDate, today_spent: f64, month_to_date: f64, t: Thresholds) -> BudgetView {
    let dim = days_in_month(today);
    let elapsed = today.day();
    let left = dim - elapsed + 1;
    let set = monthly_budget > 0.0;
    let budget = if set { monthly_budget } else { 0.0 };
    let daily_allowance = budget / dim as f64;
    let today_share = if set { today_spent / daily_allowance } else { 0.0 };
    BudgetView {
        monthly_budget: budget,
        days_in_month: dim,
        days_elapsed: elapsed,
        days_left: left,
        daily_allowance,
        today: today_spent,
        today_share,
        month_to_date,
        on_pace: daily_allowance * elapsed as f64,
        projected: month_to_date / elapsed as f64 * dim as f64,
        remaining_per_day: ((budget - month_to_date) / left as f64).max(0.0),
        ring: if set { ring(today_share, t) } else { Ring::None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }
    fn cents(x: f64) -> f64 {
        (x * 100.0).round() / 100.0
    }

    #[test]
    fn a_300_dollar_budget_in_a_31_day_month_is_9_68_a_day() {
        let v = view(300.0, d(2026, 10, 1), 0.0, 0.0, Thresholds::default());
        assert_eq!(v.days_in_month, 31);
        assert_eq!(cents(v.daily_allowance), 9.68);
    }

    #[test]
    fn february_has_28_days_and_29_in_a_leap_year() {
        assert_eq!(days_in_month(d(2026, 2, 10)), 28);
        assert_eq!(days_in_month(d(2028, 2, 10)), 29);
        assert_eq!(days_in_month(d(2100, 2, 10)), 28, "a century is not a leap year");
        assert_eq!(days_in_month(d(2000, 2, 10)), 29, "unless it divides by 400");
        assert_eq!(cents(view(280.0, d(2026, 2, 1), 0.0, 0.0, Thresholds::default()).daily_allowance), 10.0);
        assert_eq!(cents(view(290.0, d(2028, 2, 1), 0.0, 0.0, Thresholds::default()).daily_allowance), 10.0);
        assert_eq!(days_in_month(d(2026, 12, 31)), 31, "December rolls into the next year");
    }

    #[test]
    fn the_first_day_of_a_month() {
        let v = view(310.0, d(2026, 10, 1), 4.0, 4.0, Thresholds::default());
        assert_eq!((v.days_elapsed, v.days_left), (1, 31));
        assert_eq!(cents(v.on_pace), 10.0);
        assert_eq!(cents(v.projected), 124.0, "4 a day for 31 days");
        assert_eq!(cents(v.remaining_per_day), 9.87, "306 over 31 days");
    }

    #[test]
    fn the_last_day_of_a_month() {
        let v = view(310.0, d(2026, 10, 31), 12.0, 290.0, Thresholds::default());
        assert_eq!((v.days_elapsed, v.days_left), (31, 1));
        assert_eq!(cents(v.on_pace), 310.0);
        assert_eq!(cents(v.projected), 290.0, "on the last day the projection is the month to date");
        assert_eq!(cents(v.remaining_per_day), 20.0, "all that is left, for today");
    }

    #[test]
    fn over_budget_leaves_nothing_per_day_never_a_negative_figure() {
        let v = view(300.0, d(2026, 10, 20), 30.0, 350.0, Thresholds::default());
        assert_eq!(v.remaining_per_day, 0.0);
        let exact = view(300.0, d(2026, 10, 20), 0.0, 300.0, Thresholds::default());
        assert_eq!(exact.remaining_per_day, 0.0);
    }

    #[test]
    fn the_ring_takes_a_thresholds_colour_at_the_threshold() {
        let t = Thresholds::default();
        // A 31 day month: the allowance is 310 / 31 = 10.
        let at = |spent: f64| view(310.0, d(2026, 10, 15), spent, 0.0, t).ring;
        assert_eq!(at(7.9), Ring::Green, "79%");
        assert_eq!(at(8.0), Ring::Amber, "80%");
        assert_eq!(at(9.99), Ring::Amber, "99.9%");
        assert_eq!(at(10.0), Ring::Red, "100%");
        assert_eq!(at(25.0), Ring::Red);
        assert_eq!(at(0.0), Ring::Green);
    }

    #[test]
    fn thresholds_can_be_moved() {
        let t = Thresholds { amber: 0.5, red: 0.9 };
        assert_eq!(ring(0.49, t), Ring::Green);
        assert_eq!(ring(0.5, t), Ring::Amber);
        assert_eq!(ring(0.9, t), Ring::Red);
    }

    #[test]
    fn no_budget_means_no_ring_and_no_allowance_but_the_spend_still_shows() {
        let v = view(0.0, d(2026, 10, 15), 12.5, 80.0, Thresholds::default());
        assert_eq!(v.ring, Ring::None);
        assert_eq!(v.daily_allowance, 0.0);
        assert_eq!(v.today_share, 0.0);
        assert_eq!(v.today, 12.5);
        assert_eq!(v.month_to_date, 80.0);
        assert_eq!(v.remaining_per_day, 0.0);
        assert_eq!(view(-5.0, d(2026, 10, 15), 1.0, 1.0, Thresholds::default()).ring, Ring::None);
    }

    #[test]
    fn a_daylight_saving_day_is_one_calendar_day() {
        // 2026-11-01 is 25 hours long in New York; the arithmetic counts dates.
        let v = view(300.0, d(2026, 11, 1), 0.0, 0.0, Thresholds::default());
        assert_eq!((v.days_in_month, v.days_elapsed, v.days_left), (30, 1, 30));
        let w = view(300.0, d(2026, 3, 8), 0.0, 0.0, Thresholds::default());
        assert_eq!((w.days_in_month, w.days_elapsed, w.days_left), (31, 8, 24), "the 23 hour spring day");
    }
}
