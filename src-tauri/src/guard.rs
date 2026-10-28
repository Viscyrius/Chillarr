//! The background guard: a cheap check every minute while Chillarr sits in
//! the tray, and a fix when something is down. It never prompts for admin and
//! never opens a window; it tells the user through one notification per problem.

use crate::fix::{self, Trigger};
use crate::model::{AppId, Level};
use crate::status::CardState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

const TICK: Duration = Duration::from_secs(60);
/// Let services that start with Windows come up before the first check.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// "Fix automatically" from Settings.
pub struct AutoFix(pub AtomicBool);

#[derive(Default)]
pub struct Notifier {
    seen: HashMap<AppId, String>,
}

impl Notifier {
    /// True when the user should hear about this card now.
    pub fn should_notify(&mut self, card: &CardState, changed_something: bool) -> bool {
        if changed_something {
            self.seen.insert(card.id, card.line.clone());
            return true;
        }
        if !matches!(card.level, Level::Red | Level::Amber) {
            self.seen.remove(&card.id);
            return false;
        }
        if self.seen.get(&card.id) == Some(&card.line) {
            return false;
        }
        self.seen.insert(card.id, card.line.clone());
        true
    }
}

pub fn needs_attention(cards: &[CardState]) -> bool {
    cards.iter().any(|c| c.fixable && matches!(c.level, Level::Red | Level::Amber))
}

fn notify(app: &AppHandle, body: &str) {
    let _ = app.notification().builder().title("Chillarr").body(body).show();
}

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut notifier = Notifier::default();
        tokio::time::sleep(FIRST_CHECK).await;
        // Start what is down once at launch, even with "Fix automatically" off:
        // Radarr/Sonarr switched to run as the user rely on Chillarr for this.
        fix::fix_all(&app, Trigger::Guard).await;
        let mut tick = tokio::time::interval(TICK);
        let mut last_update_check: Option<std::time::Instant> = None;
        loop {
            tick.tick().await;
            if last_update_check.map_or(true, |t| t.elapsed() >= UPDATE_EVERY) {
                last_update_check = Some(std::time::Instant::now());
                for (id, v) in fix::check_updates(&app).await {
                    notify(&app, &format!("{} {v} is out. Open Chillarr to update.", id.label()));
                }
            }
            // Not a fix, so not behind "Fix automatically": without it Jellyfin's search finds nothing.
            crate::meili::keep_alive(&app).await;
            if !app.state::<AutoFix>().0.load(Ordering::Relaxed) {
                continue;
            }
            let mut cards = fix::refresh_cards(&app).await;
            let mut report = fix::FixReport::default();
            if needs_attention(&cards) {
                report = fix::fix_all(&app, Trigger::Guard).await;
                cards = report.cards.clone();
            }
            for action in &report.actions {
                notify(&app, action);
            }
            for card in &cards {
                let label = card.id.label();
                let fixed_now = report.actions.iter().any(|a| a.starts_with(label));
                if notifier.should_notify(card, false) && !fixed_now {
                    let body = report
                        .needs_button
                        .iter()
                        .find(|m| m.starts_with(label))
                        .cloned()
                        .unwrap_or_else(|| format!("{label}: {}", card.line));
                    notify(&app, &body);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(level: Level, line: &str) -> CardState {
        let fixable = matches!(level, Level::Red | Level::Amber);
        CardState { id: AppId::Sonarr, level, line: line.into(), notes: vec![], open_url: None, fixable, missing: false, update: None }
    }

    #[test]
    fn notifies_once_per_problem() {
        let mut n = Notifier::default();
        assert!(n.should_notify(&card(Level::Red, "Not running"), false));
        assert!(!n.should_notify(&card(Level::Red, "Not running"), false));
        assert!(!n.should_notify(&card(Level::Ok, "Running"), false));
        assert!(n.should_notify(&card(Level::Red, "Not running"), false));
        assert!(n.should_notify(&card(Level::Red, "Not running"), true));
    }

    #[test]
    fn a_new_problem_on_the_same_app_notifies() {
        let mut n = Notifier::default();
        assert!(n.should_notify(&card(Level::Red, "Not running"), false));
        assert!(n.should_notify(&card(Level::Amber, "1337x failing"), false));
    }

    #[test]
    fn needs_attention_counts_red_and_amber() {
        let cards = vec![card(Level::Ok, "Running"), card(Level::Amber, "x"), card(Level::Unknown, "Not installed")];
        assert!(needs_attention(&cards));
        assert!(!needs_attention(&cards[..1]));
        assert!(!needs_attention(&cards[2..]));
        let mut stuck = card(Level::Amber, "Can't see the download folder");
        stuck.fixable = false;
        assert!(!needs_attention(&[stuck]));
    }
}
