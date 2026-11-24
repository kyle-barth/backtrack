//! Automatic updates. When an installed copy starts, it fetches latest.json from the newest GitHub release. If that
//! names a newer version, it downloads the installer and checks its signature against the public key built into the
//! app (tauri.conf.json → plugins.updater.pubkey). It then waits until no game is in front and any export has
//! finished, has the window save any edit still waiting, stops the recorder, installs quietly and restarts with the
//! same arguments (so a copy started with Windows comes back in the tray). Nothing about the user is sent.
//!
//! People's own data is never part of an update: edits, favourites and export history are in
//! %APPDATA%\Backtrack\store.json and settings in settings.json, outside the install folder, and the installer runs
//! in update mode, which replaces the program files without running the uninstaller.
//!
//! What's new: the release notes ship inside the app (ui/whatsnew.js). After an update the version in the title bar
//! reads "Updated to x.y.z" until the notes are opened; whats-new.json (below) remembers how far they've been read.

use crate::{util, winutil, AppState};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering as Cmp;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Updates come as installers, so only an installed copy updates itself; the portable zip has no uninstaller.
pub fn installed() -> bool {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("uninstall.exe").is_file())).unwrap_or(false)
}

/// The version being installed, so the restarted app can say it was updated.
fn marker() -> PathBuf {
    util::data_dir().join("updated-to.txt")
}

/// Check for a newer release in the background and, if there is one, install it and restart.
pub fn check_at_launch(app: &AppHandle) {
    if !installed() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<Arc<AppState>>().inner().clone();
        let st2 = st.clone();
        // The installer replaces ffmpeg.exe, so the recorder's ffmpeg must be gone first.
        let updater = match app.updater_builder().on_before_exit(move || st2.recorder.stop()).build() {
            Ok(u) => u,
            Err(e) => return util::log("warn", &format!("update: {e}")),
        };
        let found = tauri::async_runtime::block_on(async {
            match updater.check().await? {
                Some(update) => {
                    let bytes = update.download(|_, _| {}, || {}).await?;
                    Ok::<_, tauri_plugin_updater::Error>(Some((update, bytes)))
                }
                None => Ok(None),
            }
        });
        let (update, bytes) = match found {
            Ok(Some(u)) => u,
            Ok(None) => return util::log("info", "update: up to date"),
            Err(e) => return util::log("warn", &format!("update check: {e}")),
        };
        let mut told = false;
        loop {
            // Let an export in progress finish (up to 10 minutes) rather than cut it short.
            for _ in 0..600 {
                if !st.job.running.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            // Not in the middle of a game: the restart empties the replay buffer, so a clip saved a few seconds later
            // would come out short with no explanation. If Backtrack quits first, the next launch tries again.
            if !winutil::game_in_front() {
                break;
            }
            if !told {
                util::log("info", &format!("update: {} is ready; waiting until no game is in front", update.version));
                told = true;
            }
            std::thread::sleep(Duration::from_secs(10));
        }
        // The editor saves In/Out 400 ms after a change: have it save anything still waiting, and give it a moment.
        let _ = app.emit("app-updating", &update.version);
        std::thread::sleep(Duration::from_millis(1500));
        util::log("info", &format!("update: installing {} (running {})", update.version, update.current_version));
        let _ = std::fs::write(marker(), &update.version);
        if let Err(e) = update.install(bytes) {
            let _ = std::fs::remove_file(marker());
            util::log("warn", &format!("update: {e}"));
        }
    });
}

/// After an update restarted the app, say so once in the corner, but only when it came back in the tray (started
/// with Windows) and no game is in front. When the window comes back in front of the person, its title bar says it
/// ("Updated to x.y.z"); over a game, nothing. The pop-up can't be clicked, so it doesn't offer the notes.
pub fn announce_if_updated(app: &AppHandle, background: bool) {
    let Ok(v) = std::fs::read_to_string(marker()) else { return };
    let _ = std::fs::remove_file(marker());
    let v = v.trim();
    if v != app.package_info().version.to_string() {
        return;
    }
    if !background {
        return util::log("info", &format!("update: now on {v}, window back in front"));
    }
    if winutil::game_in_front() {
        return util::log("info", &format!("update: now on {v}; no pop-up, a game is in front"));
    }
    let on = app.state::<Arc<AppState>>().settings.lock().unwrap().recorder.enabled;
    let sub = if on { "Replay buffer is back on" } else { "Running in the tray" };
    crate::rec_app::toast(app, &format!("Backtrack updated to {v}"), sub, "update");
}

// ------------------------------------------------------------------ what's new

/// How far the release notes have been read, in %APPDATA%\Backtrack\whats-new.json. It lives here rather than in
/// the page's storage so clearing WebView2's data doesn't bring old notes back, and so it's settled before any
/// window exists. The notes after `seen`, up to the version running, are unread.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
struct NotesFile {
    /// The version that ran last time.
    last_run: String,
    /// Read up to this version. "" after an update from a version that didn't keep this file (0.4.6 and earlier):
    /// which one isn't known, so the page treats the release before this one as read.
    seen: String,
    /// When this version first ran (ms since 1970): the title bar stops saying "Updated to" 14 days later.
    updated_at: u64,
}

/// What the page gets from `notes_state`.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NotesState {
    version: String,
    seen: String,
    /// The first run after a fresh install: nothing to tell.
    fresh: bool,
    updated_at: u64,
}

#[derive(Deserialize)]
pub struct NotesPatch {
    seen: Option<String>,
}

static NOTES: Mutex<Option<(NotesFile, bool)>> = Mutex::new(None);

fn notes_path() -> PathBuf {
    util::data_dir().join("whats-new.json")
}

/// "1.2.3" or "1.2.3-beta.1" → [1, 2, 3], for comparing versions.
fn vparts(v: &str) -> [u64; 3] {
    let mut out = [0; 3];
    for (i, p) in v.split(['-', '+']).next().unwrap_or("").split('.').take(3).enumerate() {
        out[i] = p.trim().parse().unwrap_or(0);
    }
    out
}

fn vcmp(a: &str, b: &str) -> Cmp {
    vparts(a).cmp(&vparts(b))
}

/// The notes file for the version that's starting, from last time's (if any). → (file, fresh install?)
/// `existing`: Backtrack ran on this PC before (a version that didn't keep the file yet).
fn notes_on_start(prev: Option<NotesFile>, version: &str, existing: bool, now: u64) -> (NotesFile, bool) {
    let new = |seen: &str| NotesFile { last_run: version.into(), seen: seen.into(), updated_at: now };
    match prev {
        None if existing => (new(""), false),
        None => (new(version), true),
        Some(f) if f.last_run == version => (f, false),
        Some(mut f) => {
            // An update, or going back to an older version: then nothing past this one counts as unread.
            if !f.seen.is_empty() && vcmp(&f.seen, version) == Cmp::Greater {
                f.seen = version.into();
            }
            f.last_run = version.into();
            f.updated_at = now;
            (f, false)
        }
    }
}

/// At start-up, before anything writes to the data folder (the log, settings): bring whats-new.json up to this version.
pub fn notes_at_start() {
    let path = notes_path();
    let prev: Option<NotesFile> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let d = util::data_dir();
    let existing = ["backtrack.log", "settings.json", "store.json", "updated-to.txt"].iter().any(|f| d.join(f).is_file());
    let (file, fresh) = notes_on_start(prev.clone(), env!("CARGO_PKG_VERSION"), existing, util::now_ms());
    if prev.as_ref() != Some(&file) {
        if let Err(e) = util::write_json_atomic(&path, &file) {
            util::log("warn", &format!("whats-new.json: {e}"));
        }
    }
    *NOTES.lock().unwrap() = Some((file, fresh));
}

fn notes_view(f: &NotesFile, fresh: bool) -> NotesState {
    NotesState { version: env!("CARGO_PKG_VERSION").into(), seen: f.seen.clone(), fresh, updated_at: f.updated_at }
}

#[tauri::command]
pub fn notes_state() -> NotesState {
    let mut g = NOTES.lock().unwrap();
    if g.is_none() {
        drop(g);
        notes_at_start();
        g = NOTES.lock().unwrap();
    }
    let (f, fresh) = g.as_ref().expect("notes state");
    notes_view(f, *fresh)
}

/// The page read the notes (up to `seen`, never past the version running).
#[tauri::command]
pub fn set_notes_state(patch: NotesPatch) -> Result<NotesState, String> {
    let _ = notes_state();
    let mut g = NOTES.lock().unwrap();
    let (f, fresh) = g.as_mut().expect("notes state");
    if let Some(seen) = patch.seen {
        let version = env!("CARGO_PKG_VERSION");
        f.seen = if vcmp(&seen, version) == Cmp::Greater { version.into() } else { seen };
        util::write_json_atomic(&notes_path(), &*f).map_err(|e| format!("Couldn't save what's new: {e}"))?;
    }
    Ok(notes_view(f, *fresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_after_a_fresh_install_an_update_and_a_downgrade() {
        // A fresh install: everything up to this version counts as read.
        let (f, fresh) = notes_on_start(None, "0.4.7", false, 1);
        assert!(fresh);
        assert_eq!((f.last_run.as_str(), f.seen.as_str(), f.updated_at), ("0.4.7", "0.4.7", 1));
        // Same version again: untouched (and not fresh any more).
        let (g, fresh) = notes_on_start(Some(f.clone()), "0.4.7", true, 2);
        assert!(!fresh);
        assert_eq!(g, f);
        // An update: what was read stays read, so 0.4.8's notes are new; and it remembers when.
        let (g, _) = notes_on_start(Some(f.clone()), "0.4.8", true, 3);
        assert_eq!((g.last_run.as_str(), g.seen.as_str(), g.updated_at), ("0.4.8", "0.4.7", 3));
        // Two updates before anyone looks: both releases are still unread.
        let (h, _) = notes_on_start(Some(g), "0.4.9", true, 4);
        assert_eq!((h.last_run.as_str(), h.seen.as_str()), ("0.4.9", "0.4.7"));
        // Back to an older version: nothing past it counts as unread.
        let read = NotesFile { last_run: "0.5.0".into(), seen: "0.5.0".into(), updated_at: 5 };
        let (g, _) = notes_on_start(Some(read), "0.4.9", true, 6);
        assert_eq!((g.last_run.as_str(), g.seen.as_str()), ("0.4.9", "0.4.9"));
    }

    #[test]
    fn notes_after_an_update_from_before_the_file() {
        // Backtrack ran here before, but that version didn't keep the file: which version isn't known ("").
        let (f, fresh) = notes_on_start(None, "0.4.7", true, 1);
        assert!(!fresh);
        assert_eq!((f.last_run.as_str(), f.seen.as_str(), f.updated_at), ("0.4.7", "", 1));
        // Another update before the notes were opened keeps it unknown rather than inventing a version.
        let (g, _) = notes_on_start(Some(f), "0.4.8", true, 2);
        assert_eq!((g.last_run.as_str(), g.seen.as_str()), ("0.4.8", ""));
    }

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(vcmp("0.4.10", "0.4.9"), Cmp::Greater);
        assert_eq!(vcmp("0.5.0", "0.4.99"), Cmp::Greater);
        assert_eq!(vcmp("0.4.7-beta.1", "0.4.7"), Cmp::Equal);
        assert_eq!(vcmp("", "0.0.1"), Cmp::Less);
    }

    /// Release check: the updater signature next to an installer verifies against the public key built into the app
    /// (tauri.conf.json), for the version being released, the way installed copies check an update, and a changed
    /// installer doesn't. The release workflow runs it before publishing:
    ///   BACKTRACK_SIGNED_FILE=<setup.exe> BACKTRACK_SIGNED_VERSION=<x.y.z> cargo test update_signature -- --ignored
    #[test]
    #[ignore]
    fn update_signature_matches_the_app_key() {
        use base64::Engine;
        let text = |b64: &str| String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64.trim()).unwrap()).unwrap();
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let pubkey = conf["plugins"]["updater"]["pubkey"].as_str().expect("plugins.updater.pubkey");
        let file = std::env::var("BACKTRACK_SIGNED_FILE").expect("BACKTRACK_SIGNED_FILE");
        let version = std::env::var("BACKTRACK_SIGNED_VERSION").expect("BACKTRACK_SIGNED_VERSION");
        let data = std::fs::read(&file).unwrap();
        let key = minisign_verify::PublicKey::decode(&text(pubkey)).unwrap();
        let sig = minisign_verify::Signature::decode(&text(&std::fs::read_to_string(format!("{file}.sig")).unwrap())).unwrap();
        key.verify(&data, &sig, true).expect("the installer's signature doesn't match the public key in tauri.conf.json");
        if let Some(signed) = sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:")) {
            assert_eq!(signed.trim_start_matches('v'), version.trim_start_matches('v'), "signed for another version");
        }
        let mut changed = data;
        let mid = changed.len() / 2;
        changed[mid] ^= 1;
        assert!(key.verify(&changed, &sig, true).is_err(), "a changed installer must not verify");
        println!("signature OK: key {} · {}", text(pubkey).lines().next().unwrap_or(""), sig.trusted_comment());
    }
}
