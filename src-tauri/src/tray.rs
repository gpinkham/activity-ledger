//! The menu bar gauge: a ring for today's spend as a share of today's allowance,
//! a small panel under it on a click, and a menu with Open and Quit.
//!
//! Off unless "Show in menu bar" is on. With it on, closing the main window hides
//! it and the app leaves the Dock (accessory mode); "Open Activity Ledger" brings
//! both back. Every 60 seconds a poll reads what the transcripts gained (gauge.rs)
//! and redraws the ring. Nothing here opens a network connection.

use crate::budget::Ring;
use crate::commands::{gauge_view, AppState};
use crate::gauge::SystemClock;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

pub const TRAY_ID: &str = "gauge";
const PANEL: &str = "tray-panel";
const PANEL_W: f64 = 320.0;
const PANEL_H: f64 = 400.0;
pub const POLL_SECS: u64 = 60;
/// The icon in pixels: a 22 point menu bar slot at 2x.
pub const ICON: u32 = 44;

fn colour(ring: Ring) -> [u8; 3] {
    match ring {
        Ring::Green => [0x34, 0xC7, 0x59],
        Ring::Amber => [0xFF, 0x9F, 0x0A],
        Ring::Red => [0xFF, 0x3B, 0x30],
        Ring::None => [0x8E, 0x8E, 0x93],
    }
}

/// The ring as RGBA pixels: a grey track, and an arc from twelve o'clock,
/// clockwise, for the share of today's allowance spent (full at 100% and above).
pub fn ring_rgba(ring: Ring, share: f64) -> Vec<u8> {
    let n = ICON as usize;
    let mut px = vec![0u8; n * n * 4];
    let c = (n as f64 - 1.0) / 2.0;
    let (outer, inner) = (c - 1.0, c - 7.0);
    let fill = if ring == Ring::None { 0.0 } else { share.clamp(0.0, 1.0) };
    let rgb = colour(ring);
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f64 - c, y as f64 - c);
            let r = (dx * dx + dy * dy).sqrt();
            // Soft edges: one pixel of falloff on each side of the band.
            let a = ((outer + 0.5 - r).min(r - (inner - 0.5))).clamp(0.0, 1.0);
            if a == 0.0 {
                continue;
            }
            // Angle from twelve o'clock, clockwise, as a share of the circle.
            let turn = (dx.atan2(-dy) / std::f64::consts::TAU).rem_euclid(1.0);
            let (rgb, alpha) = if turn < fill { (rgb, 1.0) } else { ([0x8E, 0x8E, 0x93], 0.45) };
            let i = (y * n + x) * 4;
            px[i..i + 3].copy_from_slice(&rgb);
            px[i + 3] = (a * alpha * 255.0).round() as u8;
        }
    }
    px
}

/// "Today $4.10 of $9.68 (42%)", or the spend alone when no budget is set.
pub fn tooltip(today: f64, allowance: f64, ring: Ring) -> String {
    if ring == Ring::None {
        format!("Activity Ledger: today ${today:.2} (no budget set)")
    } else {
        format!("Activity Ledger: today ${today:.2} of ${allowance:.2} ({:.0}%)", today / allowance * 100.0)
    }
}

/// Poll the transcripts and redraw the ring and its tooltip.
pub fn refresh_icon(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let state = app.state::<AppState>();
    let Ok(v) = gauge_view(&state, &SystemClock, true) else { return };
    let b = &v.budget;
    let _ = tray.set_icon(Some(Image::new_owned(ring_rgba(b.ring, b.today_share), ICON, ICON)));
    let _ = tray.set_tooltip(Some(tooltip(b.today, b.daily_allowance, b.ring)));
}

/// Show the main window and put the app back in the Dock.
pub fn show_main(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some(p) = app.get_webview_window(PANEL) {
        let _ = p.hide();
    }
}

fn toggle_panel(app: &AppHandle, rect: tauri::Rect) {
    let panel = match app.get_webview_window(PANEL) {
        Some(p) => p,
        None => {
            let Ok(p) = WebviewWindowBuilder::new(app, PANEL, WebviewUrl::App("index.html#tray".into()))
                .title("Activity Ledger budget")
                .inner_size(PANEL_W, PANEL_H)
                .resizable(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible(false)
                .build()
            else {
                return;
            };
            let hide = p.clone();
            p.on_window_event(move |e| {
                if let WindowEvent::Focused(false) = e {
                    let _ = hide.hide();
                }
            });
            p
        }
    };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        return;
    }
    // Centred under the icon.
    let scale = panel.scale_factor().unwrap_or(1.0);
    let pos = rect.position.to_physical::<f64>(scale);
    let size = rect.size.to_physical::<f64>(scale);
    let x = pos.x + size.width / 2.0 - PANEL_W * scale / 2.0;
    let y = pos.y + size.height;
    let _ = panel.set_position(tauri::PhysicalPosition::new(x.max(0.0), y));
    let _ = panel.show();
    let _ = panel.set_focus();
    // The panel reloads its figures when it is shown.
    let _ = tauri::Emitter::emit_to(app, PANEL, "gauge-shown", ());
}

/// Put the icon in the menu bar, or take it out, to match the setting.
pub fn sync(app: &AppHandle) -> tauri::Result<()> {
    let on = app.state::<AppState>().settings.lock().map(|s| s.show_in_menu_bar).unwrap_or(false);
    let present = app.tray_by_id(TRAY_ID).is_some();
    if on && !present {
        let open = MenuItem::with_id(app, "open", "Open Activity Ledger", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit Activity Ledger", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;
        TrayIconBuilder::with_id(TRAY_ID)
            .icon(Image::new_owned(ring_rgba(Ring::None, 0.0), ICON, ICON))
            .tooltip("Activity Ledger")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, e| match e.id().as_ref() {
                "open" => show_main(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, e| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = e {
                    toggle_panel(tray.app_handle(), rect);
                }
            })
            .build(app)?;
        refresh_icon(app);
    } else if !on && present {
        app.remove_tray_by_id(TRAY_ID);
        if let Some(p) = app.get_webview_window(PANEL) {
            let _ = p.close();
        }
        // With no icon left, a hidden main window would leave no way back in.
        show_main(app);
    }
    Ok(())
}

/// With the gauge on, closing the main window hides it and leaves the Dock; the
/// app keeps running in the menu bar. With it off, closing quits as before.
pub fn on_main_close(app: &AppHandle, api: &tauri::CloseRequestApi) {
    let on = app.state::<AppState>().settings.lock().map(|s| s.show_in_menu_bar).unwrap_or(false);
    if on && app.tray_by_id(TRAY_ID).is_some() {
        api.prevent_close();
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
        #[cfg(target_os = "macos")]
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

/// The 60 second poll, for the life of the app. It reads nothing while the gauge
/// is off.
pub fn start_poll(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(POLL_SECS));
        if app.tray_by_id(TRAY_ID).is_some() {
            refresh_icon(&app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(px: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * ICON as usize + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn the_arc_fills_clockwise_from_twelve_for_the_share_spent() {
        let px = ring_rgba(Ring::Amber, 0.25);
        assert_eq!(px.len(), (ICON * ICON * 4) as usize);
        let top_right = pixel(&px, 30, 6); // a little past twelve
        let bottom = pixel(&px, 22, 40); // six o'clock: past 25%
        assert_eq!(&top_right[..3], &colour(Ring::Amber));
        assert_eq!(top_right[3], 255);
        assert_eq!(&bottom[..3], &[0x8E, 0x8E, 0x93], "the track");
        assert!(bottom[3] < 255);
        assert_eq!(pixel(&px, 22, 22)[3], 0, "the middle is clear");
        assert_eq!(pixel(&px, 0, 0)[3], 0, "so are the corners");
    }

    #[test]
    fn over_the_allowance_the_ring_is_full_and_with_no_budget_it_is_only_the_track() {
        let red = ring_rgba(Ring::Red, 1.7);
        assert_eq!(&pixel(&red, 22, 40)[..3], &colour(Ring::Red));
        assert_eq!(&pixel(&red, 6, 22)[..3], &colour(Ring::Red));
        let none = ring_rgba(Ring::None, 0.9);
        assert_eq!(&pixel(&none, 30, 6)[..3], &[0x8E, 0x8E, 0x93]);
        assert!(pixel(&none, 30, 6)[3] < 255);
    }

    #[test]
    fn the_tooltip_says_the_share_or_that_no_budget_is_set() {
        assert_eq!(tooltip(4.1, 9.677, Ring::Green), "Activity Ledger: today $4.10 of $9.68 (42%)");
        assert_eq!(tooltip(4.1, 0.0, Ring::None), "Activity Ledger: today $4.10 (no budget set)");
    }
}
