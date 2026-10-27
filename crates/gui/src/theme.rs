//! Shared color palette for the GUI.
//!
//! Every view picks colors from here, so the look stays consistent and a
//! color change is made in one place. Values follow the Tailwind slate scale
//! with an indigo accent.

use eframe::egui::Color32;

// Backgrounds, darkest first.
pub const SIDEBAR_BG: Color32 = Color32::from_rgb(9, 13, 22);
pub const APP_BG: Color32 = Color32::from_rgb(11, 15, 25);
/// Terminal-like areas (log viewer) and idle widgets.
pub const SUNKEN: Color32 = Color32::from_rgb(15, 23, 42);
/// Section frames.
pub const SURFACE: Color32 = Color32::from_rgb(20, 26, 38);
/// Cards, secondary buttons and section borders (slate-800).
pub const RAISED: Color32 = Color32::from_rgb(30, 41, 59);
/// Borders of raised cards (slate-700).
pub const BORDER: Color32 = Color32::from_rgb(51, 65, 85);

// Text, brightest first.
pub const TEXT_STRONG: Color32 = Color32::from_rgb(255, 255, 255);
pub const TEXT_BODY: Color32 = Color32::from_rgb(226, 232, 240);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(148, 163, 184);
pub const TEXT_SUBTLE: Color32 = Color32::from_rgb(140, 155, 175);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(110, 125, 150);

// Accent and states.
pub const ACCENT: Color32 = Color32::from_rgb(99, 102, 241);
pub const SUCCESS: Color32 = Color32::from_rgb(34, 197, 94);
pub const SUCCESS_SURFACE: Color32 = Color32::from_rgb(15, 45, 25);
pub const DANGER: Color32 = Color32::from_rgb(239, 68, 68);
pub const DANGER_SOFT: Color32 = Color32::from_rgb(248, 180, 180);
pub const DANGER_SURFACE: Color32 = Color32::from_rgb(45, 15, 20);
pub const DANGER_BORDER: Color32 = Color32::from_rgb(127, 29, 29);
/// Fill of destructive buttons (delete, clear).
pub const DANGER_BUTTON: Color32 = Color32::from_rgb(185, 28, 28);

// Status badges: (text, background).
pub const BADGE_SUCCESS_TEXT: Color32 = Color32::from_rgb(187, 247, 208);
pub const BADGE_SUCCESS_BG: Color32 = Color32::from_rgb(20, 83, 45);
pub const BADGE_FAILED_TEXT: Color32 = Color32::from_rgb(254, 202, 202);
pub const BADGE_FAILED_BG: Color32 = DANGER_BORDER;
pub const BADGE_PENDING_TEXT: Color32 = Color32::from_rgb(254, 243, 199);
pub const BADGE_PENDING_BG: Color32 = Color32::from_rgb(120, 53, 4);

// Log levels.
pub const LOG_TEXT: Color32 = Color32::from_rgb(203, 213, 225);
pub const LOG_ERROR: Color32 = Color32::from_rgb(248, 113, 113);
pub const LOG_WARN: Color32 = Color32::from_rgb(251, 146, 60);
pub const LOG_INFO: Color32 = Color32::from_rgb(94, 234, 212);
pub const LOG_DEBUG: Color32 = TEXT_MUTED;
