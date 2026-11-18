//! The menu bar gauge's budget arithmetic: a monthly budget spread evenly over the
//! calendar month, read against what this Mac's Claude Code transcripts cost.
//!
//! Every input is a local calendar date, never a duration, so a 23 or 25 hour
//! daylight saving day is still one day. Today counts as a day elapsed and as a
//! day left: it has started, and there is still time to spend in it.
//!
//! An account can spread its budget over Monday to Friday instead (off by
//! default). Weekend spend still counts in the month to date; the weekend has no
//! allowance of its own, so its ring is grey. Public holidays are not known.
//!
//! The ring measures today against what is left: the budget minus what was spent
//! before today, spread over the days left with today included (`today_allowance`).
//! It is fixed for the day, so spending today does not shrink its own target. The
//! even spread (`daily_allowance`) stays for the pace line and the jump check. Once
//! the budget was used up before today the ring is full red with no share
//! (`used_up`) until the 1st.

use chrono::{Datelike, Duration, NaiveDate};
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
    /// A Saturday or Sunday on a weekdays-only budget: no allowance to measure.
    Weekend,
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
    /// The budget is spread over Monday to Friday, and the day counts below are
    /// weekdays.
    pub weekdays_only: bool,
    pub days_in_month: u32,
    /// Including today (when it counts).
    pub days_elapsed: u32,
    /// Including today (when it counts).
    pub days_left: u32,
    /// Weekdays only, before the month's first weekday: there is no pace to project from.
    pub no_weekday_yet: bool,
    /// The budget spread evenly over the month's days (or its weekdays). The pace
    /// line and the jump check use it.
    pub daily_allowance: f64,
    /// What the ring measures today against: the budget minus the spend before
    /// today, over the days left (today included). Weekdays only, on a weekend, or
    /// with the budget used up: the even spread, and it is not shown.
    pub today_allowance: f64,
    /// The budget was all spent before today: the ring is full red, with no share.
    pub used_up: bool,
    pub today: f64,
    /// Today's spend as a share of today's allowance (1.0 = all of it). 0 when the
    /// budget is used up, so it is never infinite.
    pub today_share: f64,
    pub month_to_date: f64,
    /// What an even pace would have spent by the end of today.
    pub on_pace: f64,
    /// Month to date over days elapsed, times the days in the month. Before the
    /// first weekday of a weekdays-only month, the month to date.
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

/// Less than half a cent left counts as nothing left. Sums of many small costs
/// carry float error (a cent added 30,000 times is 299.99999999987), and a
/// remainder like 1.3e-10 would make a cent spent today a share in the billions.
pub const NOTHING_LEFT: f64 = 0.005;

/// Today's allowance, whether the budget was used up before today, and today's
/// share, for a day that has an allowance (`left` counts today, so it is at least 1).
fn against_what_is_left(budget: f64, today_spent: f64, month_to_date: f64, left: u32, daily_allowance: f64) -> (f64, bool, f64) {
    let left_before_today = budget - (month_to_date - today_spent).max(0.0);
    if left_before_today < NOTHING_LEFT {
        return (daily_allowance, true, 0.0);
    }
    let allowance = left_before_today / left as f64;
    (allowance, false, today_spent / allowance)
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
    let (today_allowance, used_up, today_share) =
        if set { against_what_is_left(budget, today_spent, month_to_date, left, daily_allowance) } else { (0.0, false, 0.0) };
    BudgetView {
        monthly_budget: budget,
        weekdays_only: false,
        days_in_month: dim,
        days_elapsed: elapsed,
        days_left: left,
        no_weekday_yet: false,
        daily_allowance,
        today_allowance,
        used_up,
        today: today_spent,
        today_share,
        month_to_date,
        on_pace: daily_allowance * elapsed as f64,
        projected: month_to_date / elapsed as f64 * dim as f64,
        remaining_per_day: ((budget - month_to_date) / left as f64).max(0.0),
        ring: match (set, used_up) {
            (false, _) => Ring::None,
            (true, true) => Ring::Red,
            (true, false) => ring(today_share, t),
        },
    }
}

/// Monday to Friday.
pub fn is_weekday(d: NaiveDate) -> bool {
    d.weekday().number_from_monday() <= 5
}

/// The weekdays from `from` to `to`, both included; 0 when `to` is before `from`.
pub fn weekdays_between(from: NaiveDate, to: NaiveDate) -> u32 {
    from.iter_days().take_while(|d| *d <= to).filter(|d| is_weekday(*d)).count() as u32
}

pub fn last_of_month(d: NaiveDate) -> NaiveDate {
    first_of_month(d) + Duration::days(days_in_month(d) as i64 - 1)
}

/// Monday to Friday in `d`'s month: 20 to 23.
pub fn weekdays_in_month(d: NaiveDate) -> u32 {
    weekdays_between(first_of_month(d), last_of_month(d))
}

/// view, with the budget spread over the month's weekdays when `weekdays_only`.
/// Then the day counts are weekdays: today counts as elapsed and as left only on
/// a weekday. Weekend spend stays in the month to date, so it lowers what is left
/// for each weekday, and it is spread over the weekdays in the projection.
pub fn view_for(monthly_budget: f64, today: NaiveDate, today_spent: f64, month_to_date: f64, t: Thresholds, weekdays_only: bool) -> BudgetView {
    if !weekdays_only {
        return view(monthly_budget, today, today_spent, month_to_date, t);
    }
    let (first, last) = (first_of_month(today), last_of_month(today));
    let in_month = weekdays_between(first, last);
    let elapsed = weekdays_between(first, today);
    let left = weekdays_between(today, last);
    let set = monthly_budget > 0.0;
    let budget = if set { monthly_budget } else { 0.0 };
    let daily_allowance = budget / in_month as f64;
    let weekend = !is_weekday(today);
    // A weekday is in `left`, so there is no division by zero. A weekend has no
    // allowance: its spend lowers the next weekday's instead.
    let (today_allowance, used_up, today_share) = match (set, weekend) {
        (false, _) => (0.0, false, 0.0),
        (true, true) => (daily_allowance, false, 0.0),
        (true, false) => against_what_is_left(budget, today_spent, month_to_date, left, daily_allowance),
    };
    BudgetView {
        monthly_budget: budget,
        weekdays_only: true,
        days_in_month: in_month,
        days_elapsed: elapsed,
        days_left: left,
        no_weekday_yet: elapsed == 0,
        daily_allowance,
        today_allowance,
        used_up,
        today: today_spent,
        today_share,
        month_to_date,
        on_pace: daily_allowance * elapsed as f64,
        projected: if elapsed == 0 { month_to_date } else { month_to_date / elapsed as f64 * in_month as f64 },
        remaining_per_day: if left == 0 { 0.0 } else { ((budget - month_to_date) / left as f64).max(0.0) },
        ring: match (set, weekend, used_up) {
            (false, _, _) => Ring::None,
            (true, true, _) => Ring::Weekend,
            (true, false, true) => Ring::Red,
            (true, false, false) => ring(today_share, t),
        },
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
        // The first day of a 31 day month: today's allowance is 310 / 31 = 10.
        let at = |spent: f64| view(310.0, d(2026, 10, 1), spent, spent, t).ring;
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

    fn wk(budget: f64, today: NaiveDate, spent: f64, mtd: f64) -> BudgetView {
        view_for(budget, today, spent, mtd, Thresholds::default(), true)
    }

    #[test]
    fn months_have_20_to_23_weekdays() {
        assert_eq!(weekdays_in_month(d(2026, 11, 16)), 21, "November 2026 starts on a Sunday");
        assert_eq!(weekdays_in_month(d(2026, 2, 1)), 20, "a 28 day February is 4 whole weeks");
        assert_eq!(weekdays_in_month(d(2028, 2, 1)), 21, "the leap day is a Tuesday");
        assert_eq!(weekdays_in_month(d(2026, 12, 1)), 23, "31 days from a Tuesday");
        assert_eq!(weekdays_in_month(d(2026, 8, 1)), 21, "31 days from a Saturday");
        assert_eq!(weekdays_in_month(d(2026, 10, 1)), 22);
        for m in 1..=12 {
            let n = weekdays_in_month(d(2027, m, 1));
            assert!((20..=23).contains(&n), "2027-{m}: {n}");
        }
    }

    #[test]
    fn off_by_default_and_off_is_the_calendar_spread() {
        assert!(!crate::settings::AccountSettings::default().weekdays_only);
        let t = Thresholds::default();
        assert_eq!(view_for(300.0, d(2026, 11, 16), 5.0, 90.0, t, false), view(300.0, d(2026, 11, 16), 5.0, 90.0, t));
        assert!(!view(300.0, d(2026, 11, 16), 5.0, 90.0, t).weekdays_only);
    }

    #[test]
    fn the_allowance_is_the_budget_over_the_months_real_weekdays() {
        let v = wk(1000.0, d(2026, 11, 16), 0.0, 0.0);
        assert!(v.weekdays_only);
        assert_eq!(v.days_in_month, 21);
        assert_eq!(cents(v.daily_allowance), 47.62, "1000 over 21, not over 22 or 30");
        assert_eq!(cents(wk(1000.0, d(2026, 12, 1), 0.0, 0.0).daily_allowance), 43.48, "23 weekdays");
        assert_eq!(cents(wk(1000.0, d(2026, 2, 2), 0.0, 0.0).daily_allowance), 50.0, "20 weekdays");
    }

    #[test]
    fn a_weekday_counts_itself_as_elapsed_and_left() {
        // Monday 16 November 2026: the 11th weekday, with 11 left counting today.
        let v = wk(2100.0, d(2026, 11, 16), 50.0, 1000.0);
        assert_eq!((v.days_elapsed, v.days_left), (11, 11));
        assert_eq!(cents(v.on_pace), 1100.0, "100 a weekday for 11 weekdays");
        assert_eq!(cents(v.projected), 1909.09, "1000 over 11 weekdays, times 21");
        assert_eq!(cents(v.remaining_per_day), 100.0, "1100 over 11 weekdays");
        assert_eq!(cents(v.today_allowance), 104.55, "1150 left before today, over 11 weekdays");
        assert_eq!(cents(v.today_share), 0.48);
        assert_eq!(v.ring, Ring::Green);
        assert!(!v.no_weekday_yet);
    }

    #[test]
    fn the_weekend_ring_is_grey_and_its_spend_still_counts() {
        // Saturday 21 November 2026: 15 weekdays gone, 6 left.
        let v = wk(2100.0, d(2026, 11, 21), 400.0, 1500.0);
        assert_eq!(v.ring, Ring::Weekend, "not red, though there is no weekend allowance");
        assert_eq!(v.today_share, 0.0);
        assert_eq!(v.today, 400.0, "the dollars still show");
        assert_eq!(v.month_to_date, 1500.0, "and count in the month");
        assert_eq!((v.days_elapsed, v.days_left), (15, 6));
        assert_eq!(cents(v.remaining_per_day), 100.0, "600 left over 6 weekdays: the weekend lowered it");
        assert_eq!(cents(v.projected), 2100.0, "1500 over 15 weekdays, times 21");
        assert_eq!(wk(2100.0, d(2026, 11, 22), 0.0, 1500.0).ring, Ring::Weekend, "Sunday too");
        assert_eq!(wk(0.0, d(2026, 11, 21), 400.0, 1500.0).ring, Ring::None, "no budget: no ring at all");
    }

    #[test]
    fn a_weekday_over_its_allowance_is_red_as_usual() {
        let t = Thresholds::default();
        // Monday 2 November 2026, the month's first weekday: 2100 / 21 = 100.
        let at = |spent: f64| view_for(2100.0, d(2026, 11, 2), spent, spent, t, true).ring;
        assert_eq!(at(79.0), Ring::Green);
        assert_eq!(at(80.0), Ring::Amber);
        assert_eq!(at(100.0), Ring::Red);
    }

    #[test]
    fn a_month_that_starts_on_a_weekend_has_no_weekday_yet_and_never_divides_by_zero() {
        // August 2026 starts on a Saturday.
        for day in [1, 2] {
            let v = wk(2100.0, d(2026, 8, day), 30.0, 45.0);
            assert!(v.no_weekday_yet);
            assert_eq!(v.days_elapsed, 0);
            assert_eq!(v.days_left, 21);
            assert_eq!(v.projected, 45.0, "the month to date, not a division by zero");
            assert!(v.projected.is_finite() && v.remaining_per_day.is_finite() && v.on_pace.is_finite());
            assert_eq!(v.on_pace, 0.0);
            assert_eq!(cents(v.remaining_per_day), 97.86, "2055 over 21 weekdays");
            assert_eq!(v.ring, Ring::Weekend);
        }
        let monday = wk(2100.0, d(2026, 8, 3), 30.0, 75.0);
        assert!(!monday.no_weekday_yet);
        assert_eq!(monday.days_elapsed, 1);
        assert_eq!(cents(monday.projected), 1575.0, "75 on the first weekday, times 21");
    }

    #[test]
    fn a_month_that_ends_on_a_weekend_has_no_weekdays_left() {
        // Saturday 31 October 2026; the last weekday was Friday the 30th.
        let v = wk(2200.0, d(2026, 10, 31), 10.0, 1800.0);
        assert_eq!((v.days_elapsed, v.days_left), (22, 0));
        assert_eq!(v.remaining_per_day, 0.0, "nothing to spread it over, and no division by zero");
        assert_eq!(cents(v.projected), 1800.0);
        let friday = wk(2200.0, d(2026, 10, 30), 10.0, 1700.0);
        assert_eq!(friday.days_left, 1);
        assert_eq!(cents(friday.remaining_per_day), 500.0);
    }

    #[test]
    fn weekdays_only_in_february_and_a_leap_year() {
        let v = wk(2000.0, d(2026, 2, 27), 0.0, 1900.0);
        assert_eq!((v.days_in_month, v.days_elapsed, v.days_left), (20, 20, 1));
        let leap = wk(2100.0, d(2028, 2, 29), 0.0, 0.0);
        assert_eq!((leap.days_in_month, leap.days_elapsed, leap.days_left), (21, 21, 1), "the 29th is a Tuesday and counts");
    }

    #[test]
    fn weekdays_only_over_the_daylight_saving_weeks() {
        // 2026-11-01 (a Sunday, 25 hours) and 2026-03-08 (a Sunday, 23 hours).
        let fall = wk(2100.0, d(2026, 11, 2), 0.0, 0.0);
        assert_eq!((fall.days_elapsed, fall.days_left), (1, 21));
        let spring = wk(2200.0, d(2026, 3, 9), 0.0, 0.0);
        assert_eq!((spring.days_in_month, spring.days_elapsed, spring.days_left), (22, 6, 17));
    }

    #[test]
    fn weekdays_over_budget_leaves_nothing_per_day() {
        assert_eq!(wk(2100.0, d(2026, 11, 16), 0.0, 2500.0).remaining_per_day, 0.0);
    }

    #[test]
    fn a_daylight_saving_day_is_one_calendar_day() {
        // 2026-11-01 is 25 hours long in New York; the arithmetic counts dates.
        let v = view(300.0, d(2026, 11, 1), 0.0, 0.0, Thresholds::default());
        assert_eq!((v.days_in_month, v.days_elapsed, v.days_left), (30, 1, 30));
        let w = view(300.0, d(2026, 3, 8), 0.0, 0.0, Thresholds::default());
        assert_eq!((w.days_in_month, w.days_elapsed, w.days_left), (31, 8, 24), "the 23 hour spring day");
    }

    // The worked examples in the spec (HAG-208): $300 in December 2026, 31 days,
    // an even spread of $9.68.

    #[test]
    fn case_a_a_heavy_first_week_lowers_todays_allowance() {
        // Day 8: $120 spent before today, $7 today.
        let v = view(300.0, d(2026, 12, 8), 7.0, 127.0, Thresholds::default());
        assert_eq!(cents(v.today_allowance), 7.5, "180 left over the 24 days left, today included");
        assert_eq!(cents(v.today_share), 0.93);
        assert_eq!(v.ring, Ring::Amber);
        assert!(!v.used_up);
        assert_eq!(cents(v.daily_allowance), 9.68, "the even spread is unchanged");
        assert_eq!(cents(v.remaining_per_day), 7.21, "and so is what is left per day");
    }

    #[test]
    fn spending_today_does_not_change_todays_allowance() {
        for today in [0.0, 7.0, 20.0] {
            let v = view(300.0, d(2026, 12, 8), today, 120.0 + today, Thresholds::default());
            assert_eq!(cents(v.today_allowance), 7.5, "${today} today");
        }
    }

    #[test]
    fn case_b_a_budget_used_up_before_today_is_full_red_with_no_share() {
        // Day 20: $310 spent before today, $5 today.
        let v = view(300.0, d(2026, 12, 20), 5.0, 315.0, Thresholds::default());
        assert!(v.used_up);
        assert_eq!(v.ring, Ring::Red);
        assert_eq!(v.today_share, 0.0, "no share at all");
        assert!(v.today_share.is_finite() && v.today_allowance.is_finite());
        assert_eq!(v.remaining_per_day, 0.0);
        assert_eq!(cents(v.projected), 488.25, "already red in every version");
        let exact = view(300.0, d(2026, 12, 20), 0.0, 300.0, Thresholds::default());
        assert!(exact.used_up, "exactly all of it, before today, is used up too");
        assert_eq!(exact.today_share, 0.0);
        let light = view(300.0, d(2026, 12, 20), 0.0, 310.0, Thresholds::default());
        assert_eq!((light.ring, light.used_up), (Ring::Red, true), "a day with nothing spent is still red");
    }

    #[test]
    fn under_half_a_cent_left_before_today_is_used_up() {
        let summed: f64 = (0..30_000).map(|_| 0.01).sum();
        assert!(summed < 300.0, "the float sum falls just short: {summed}");
        let v = view(300.0, d(2026, 12, 20), 0.01, summed + 0.01, Thresholds::default());
        assert!(v.used_up, "a remainder of {} is nothing left", 300.0 - summed);
        assert_eq!((v.ring, v.today_share), (Ring::Red, 0.0));
        let close = view(300.0, d(2026, 12, 20), 0.01, 299.999 + 0.01, Thresholds::default());
        assert!(close.used_up, "a tenth of a cent left");
        let cent = view(300.0, d(2026, 12, 20), 0.0, 299.99, Thresholds::default());
        assert!(!cent.used_up, "a whole cent is still something left");
        assert!(cent.today_share.is_finite());
    }

    #[test]
    fn crossing_the_budget_during_today_is_ordinary_red_with_a_share() {
        // $295 spent before today, $10 today: $5 was left over 12 days.
        let v = view(300.0, d(2026, 12, 20), 10.0, 305.0, Thresholds::default());
        assert!(!v.used_up);
        assert_eq!(v.ring, Ring::Red);
        assert_eq!(cents(v.today_allowance), 0.42);
        assert_eq!(v.today_share.round(), 24.0, "10 over 5 / 12");
    }

    #[test]
    fn the_first_day_is_the_even_spread_and_the_last_day_is_all_that_is_left() {
        let first = view(300.0, d(2026, 12, 1), 3.0, 3.0, Thresholds::default());
        assert_eq!(first.today_allowance, first.daily_allowance);
        let last = view(300.0, d(2026, 12, 31), 12.0, 272.0, Thresholds::default());
        assert_eq!(cents(last.today_allowance), 40.0, "$40 was left this morning");
        assert_eq!(cents(last.today_share), 0.3);
    }

    #[test]
    fn underspending_early_raises_todays_allowance_with_no_cap() {
        // $20 spent by the end of day 10.
        let v = view(300.0, d(2026, 12, 11), 0.0, 20.0, Thresholds::default());
        assert_eq!(cents(v.today_allowance), 13.33, "280 over 21 days");
        assert_eq!(cents(v.daily_allowance), 9.68);
    }

    #[test]
    fn weekdays_only_measures_a_weekday_against_what_is_left_over_the_weekdays_left() {
        // December 2026: 23 weekdays, an even 13.04. Tuesday the 8th is the 6th
        // weekday, with 18 left counting today.
        let v = wk(300.0, d(2026, 12, 8), 4.0, 104.0);
        assert_eq!(cents(v.daily_allowance), 13.04);
        assert_eq!(v.days_left, 18);
        assert_eq!(cents(v.today_allowance), 11.11, "200 left before today over 18 weekdays");
        assert_eq!(v.ring, Ring::Green);
    }

    #[test]
    fn weekend_spend_keeps_a_grey_ring_and_lowers_mondays_allowance() {
        // Saturday 12 December: $30 on the weekend after $100 by Friday.
        let sat = wk(300.0, d(2026, 12, 12), 30.0, 130.0);
        assert_eq!(sat.ring, Ring::Weekend);
        assert!(!sat.used_up);
        assert_eq!(sat.today_share, 0.0);
        // Monday the 14th: 14 weekdays left, today included.
        let monday = wk(300.0, d(2026, 12, 14), 0.0, 130.0);
        assert_eq!(cents(monday.today_allowance), 12.14, "170 over 14, lowered by the weekend");
        let quiet = wk(300.0, d(2026, 12, 14), 0.0, 100.0);
        assert!(monday.today_allowance < quiet.today_allowance);
        let over = wk(300.0, d(2026, 12, 12), 50.0, 320.0);
        assert_eq!((over.ring, over.used_up), (Ring::Weekend, false), "a weekend stays grey even over budget");
    }

    #[test]
    fn no_weekday_yet_stays_grey_and_the_first_weekday_spreads_over_all_of_them() {
        // August 2026 starts on a Saturday; $45 spent on the weekend.
        let sat = wk(2100.0, d(2026, 8, 1), 30.0, 45.0);
        assert_eq!(sat.ring, Ring::Weekend);
        let monday = wk(2100.0, d(2026, 8, 3), 0.0, 45.0);
        assert_eq!(cents(monday.today_allowance), 97.86, "2055 over all 21 weekdays");
    }

    #[test]
    fn a_weekend_after_the_last_weekday_never_divides() {
        // Saturday 31 October 2026: no weekdays left.
        let v = wk(2200.0, d(2026, 10, 31), 10.0, 1800.0);
        assert_eq!(v.days_left, 0);
        assert_eq!(v.ring, Ring::Weekend);
        assert!(v.today_allowance.is_finite() && v.today_share.is_finite());
        let spent = wk(2200.0, d(2026, 10, 31), 10.0, 2300.0);
        assert_eq!((spent.ring, spent.used_up), (Ring::Weekend, false));
    }

    #[test]
    fn a_weekday_with_the_budget_used_up_is_full_red_with_no_share() {
        let v = wk(300.0, d(2026, 12, 14), 2.0, 302.0);
        assert_eq!((v.ring, v.used_up, v.today_share), (Ring::Red, true, 0.0));
    }

    #[test]
    fn no_budget_has_no_allowance_of_either_kind() {
        let v = view(0.0, d(2026, 12, 8), 7.0, 127.0, Thresholds::default());
        assert_eq!((v.ring, v.today_allowance, v.used_up), (Ring::None, 0.0, false));
        let w = wk(0.0, d(2026, 12, 8), 7.0, 127.0);
        assert_eq!((w.ring, w.today_allowance, w.used_up), (Ring::None, 0.0, false));
    }
}
