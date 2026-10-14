//! Print the "By activity" table for this machine's transcripts: categories,
//! counts and costs only. Uses a throwaway cache DB; the app's own cache is untouched.
//!
//!   cargo run --example activity_sample

use activity_ledger_lib::{commands::do_refresh, metrics::Filters, pricing::PriceTable, settings::AppSettings, store::Store};

fn main() {
    let home = dirs::home_dir().expect("home dir");
    let db = std::env::temp_dir().join(format!("al-activity-sample-{}.sqlite", std::process::id()));
    let mut store = Store::open(&db).expect("open db");
    let summary = do_refresh(&mut store, &home.join(".claude").join("projects")).expect("refresh");
    let settings = AppSettings::default();
    let r = store.activity(&PriceTable::seed(), &Filters::default(), &settings).expect("activity");

    println!("files {} | est. API cost (list price) ${:.2} | overhead {:.1}% | local candidates {:.1}%",
        summary.files_scanned, r.totals.cost_usd,
        100.0 * r.totals.overhead_usd / r.totals.cost_usd.max(1e-9), 100.0 * r.totals.local_candidate_usd / r.totals.cost_usd.max(1e-9));
    println!("{:<18} {:<9} {:>8} {:>7} {:>11} {:>6} {:>9} {:>9} {:>8}  local?", "activity", "group", "segments", "calls", "cost", "share", "ctx p50", "ctx p95", "out p95");
    for x in &r.rows {
        println!("{:<18} {:<9} {:>8} {:>7} {:>11} {:>5.1}% {:>9} {:>9} {:>8}  {}",
            x.activity, x.group, x.segments, x.calls, format!("${:.2}", x.cost_usd), 100.0 * x.share,
            x.ctx_p50, x.ctx_p95, x.out_p95, if x.local_candidate { "candidate" } else { "no" });
    }
    println!("check vs Claude Code: {} sessions, ours ${:.2} vs its ${:.2}", r.reconcile.sessions, r.reconcile.ours_usd, r.reconcile.claude_code_usd);
    if !r.unknown_models.is_empty() { println!("unknown models (default price): {}", r.unknown_models.join(", ")); }
    if std::env::var_os("KEEP_DB").is_some() { eprintln!("db kept: {}", db.display()); } else { std::fs::remove_file(&db).ok(); }
}
