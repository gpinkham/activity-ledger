//! The menu bar gauge: a ring for today's spend as a share of today's allowance,
//! and a small panel under it on any click. The panel holds Open, Save weekly
//! report and Quit.
//!
//! The icon has no native menu. With one attached, tray-icon 0.24 gives the
//! status item that menu for the life of the icon, and on macOS 27 AppKit then
//! opens it on every click before our handler sees the click (tauri issue
//! 16035), so the panel never shows. With no menu, every click reaches
//! `on_tray_icon_event`, whichever button it was.
//!
//! Off unless "Show in menu bar" is on. With it on, closing the main window hides
//! it and the app leaves the Dock (accessory mode); "Open Activity Ledger" brings
//! both back. Every 60 seconds a poll reads what the transcripts gained (gauge.rs)
//! and redraws the ring. Nothing here opens a network connection.

use crate::budget::Ring;
use crate::commands::{gauge_view, AppState};
use crate::gauge::SystemClock;
use tauri::image::Image;
use tauri::tray::{MouseButtonState, TrayIconBuilder, TrayIconEvent};
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
        hide_panel(&p);
    }
}

/// Hide the panel and tell it so, so it stops refreshing until it is shown again.
fn hide_panel(panel: &tauri::WebviewWindow) {
    let _ = panel.hide();
    let _ = tauri::Emitter::emit_to(panel, PANEL, "gauge-hidden", ());
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
                // The icon has no menu to fall back on, so a click must still lead
                // somewhere: the main window, which has the app menu and Cmd+Q.
                show_main(app);
                return;
            };
            let hide = p.clone();
            p.on_window_event(move |e| {
                if let WindowEvent::Focused(false) = e {
                    hide_panel(&hide);
                }
            });
            p
        }
    };
    if panel.is_visible().unwrap_or(false) {
        hide_panel(&panel);
        return;
    }
    // Centred under the icon, kept on the icon's screen.
    let scale = panel.scale_factor().unwrap_or(1.0);
    let pos = rect.position.to_physical::<f64>(scale);
    let size = rect.size.to_physical::<f64>(scale);
    let centre = pos.x + size.width / 2.0;
    let screen = panel
        .monitor_from_point(centre, pos.y)
        .ok()
        .flatten()
        .or_else(|| panel.current_monitor().ok().flatten())
        .map(|m| (m.position().x as f64, m.size().width as f64));
    let x = panel_x(centre, PANEL_W * scale, screen);
    let screen_y = panel
        .monitor_from_point(centre, pos.y)
        .ok()
        .flatten()
        .map(|m| (m.position().y as f64, m.size().height as f64));
    let y = panel_y(pos.y, size.height, PANEL_H * scale, screen_y);
    let _ = panel.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = panel.show();
    let _ = panel.set_focus();
    // The panel reloads its figures when it is shown.
    let _ = tauri::Emitter::emit_to(app, PANEL, "gauge-shown", ());
}

/// Where the icon is, when `e` is the release of any mouse button on it: the
/// click that opens or closes the panel. Presses, double clicks, moves, enter
/// and leave are not.
fn panel_click(e: &TrayIconEvent) -> Option<tauri::Rect> {
    match e {
        TrayIconEvent::Click { button_state: MouseButtonState::Up, rect, .. } => Some(*rect),
        _ => None,
    }
}

/// The panel's left edge, in physical pixels: centred on `centre`, and within the
/// screen `(left, width)` when it is known, else at or right of 0.
fn panel_x(centre: f64, panel_w: f64, screen: Option<(f64, f64)>) -> f64 {
    let x = centre - panel_w / 2.0;
    match screen {
        Some((left, width)) => x.min(left + width - panel_w).max(left),
        None => x.max(0.0),
    }
}

/// The panel's top edge, in physical pixels: under the icon (a menu bar at the
/// top, as on macOS), or above it when the icon is in the lower half of its
/// screen `(top, height)` (a taskbar at the bottom, as on Windows).
fn panel_y(icon_y: f64, icon_h: f64, panel_h: f64, screen: Option<(f64, f64)>) -> f64 {
    match screen {
        Some((top, height)) if icon_y > top + height / 2.0 => (icon_y - panel_h).max(top),
        _ => icon_y + icon_h,
    }
}

/// Put the icon in the menu bar, or take it out, to match the setting.
pub fn sync(app: &AppHandle) -> tauri::Result<()> {
    let on = app.state::<AppState>().settings.lock().map(|s| s.show_in_menu_bar).unwrap_or(false);
    let present = app.tray_by_id(TRAY_ID).is_some();
    if on && !present {
        // No .menu(): see the note at the top. Left, right or Control click, the
        // release of any button opens or closes the panel.
        TrayIconBuilder::with_id(TRAY_ID)
            .icon(Image::new_owned(ring_rgba(Ring::None, 0.0), ICON, ICON))
            .tooltip("Activity Ledger")
            .on_tray_icon_event(|tray, e| {
                if let Some(rect) = panel_click(&e) {
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
    use tauri::tray::{MouseButton, TrayIconId};
    use tauri::{PhysicalPosition, Position, Rect, Size};

    fn icon_rect() -> Rect {
        Rect { position: Position::Physical(PhysicalPosition::new(900, 0)), size: Size::Physical(tauri::PhysicalSize::new(44, 48)) }
    }

    fn click(button: MouseButton, button_state: MouseButtonState) -> TrayIconEvent {
        TrayIconEvent::Click { id: TrayIconId::new(TRAY_ID), position: PhysicalPosition::new(910.0, 10.0), rect: icon_rect(), button, button_state }
    }

    #[test]
    fn the_release_of_any_button_opens_the_panel() {
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            let rect = panel_click(&click(button, MouseButtonState::Up));
            assert!(rect.is_some(), "{button:?} release");
            let Position::Physical(p) = rect.unwrap().position else { panic!("kept as given") };
            assert_eq!(p, PhysicalPosition::new(900, 0), "the panel goes under the icon that was clicked");
        }
    }

    #[test]
    fn presses_double_clicks_and_hovers_do_not() {
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            assert!(panel_click(&click(button, MouseButtonState::Down)).is_none(), "{button:?} press");
        }
        let id = TrayIconId::new(TRAY_ID);
        let position = PhysicalPosition::new(910.0, 10.0);
        let rect = icon_rect();
        assert!(panel_click(&TrayIconEvent::DoubleClick { id: id.clone(), position, rect, button: MouseButton::Left }).is_none());
        assert!(panel_click(&TrayIconEvent::Enter { id: id.clone(), position, rect }).is_none());
        assert!(panel_click(&TrayIconEvent::Move { id: id.clone(), position, rect }).is_none());
        assert!(panel_click(&TrayIconEvent::Leave { id, position, rect }).is_none());
    }

    #[test]
    fn the_panel_stays_on_the_icons_screen() {
        let screen = Some((0.0, 1440.0));
        assert_eq!(panel_x(700.0, 300.0, screen), 550.0, "centred under the icon");
        assert_eq!(panel_x(1430.0, 300.0, screen), 1140.0, "an icon near the right edge: the panel ends at the edge");
        assert_eq!(panel_x(20.0, 300.0, screen), 0.0, "near the left edge");
        // A second screen to the right of the first.
        let second = Some((1440.0, 1920.0));
        assert_eq!(panel_x(3350.0, 600.0, second), 2760.0);
        assert_eq!(panel_x(1450.0, 600.0, second), 1440.0);
        assert_eq!(panel_x(5000.0, 300.0, None), 4850.0, "no screen known: only kept at or right of 0");
        assert_eq!(panel_x(10.0, 300.0, None), 0.0);
    }

    #[test]
    fn the_panel_opens_under_a_top_bar_and_above_a_bottom_taskbar() {
        assert_eq!(panel_y(0.0, 24.0, 400.0, Some((0.0, 900.0))), 24.0, "menu bar: under the icon");
        assert_eq!(panel_y(0.0, 24.0, 400.0, None), 24.0, "no screen known: under, as before");
        assert_eq!(panel_y(1040.0, 40.0, 400.0, Some((0.0, 1080.0))), 640.0, "taskbar: above the icon");
        assert_eq!(panel_y(2120.0, 40.0, 400.0, Some((1080.0, 1080.0))), 1720.0, "a second screen below the first");
    }

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
