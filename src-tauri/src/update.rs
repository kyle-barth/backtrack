//! Automatic updates. When an installed copy starts, it fetches latest.json from the newest GitHub release. If that
//! names a newer version, it downloads the installer and checks its signature against the public key built into the
//! app (tauri.conf.json → plugins.updater.pubkey). It then waits for any export to finish and for the window to save
//! any edit still waiting, stops the recorder, installs quietly and restarts with the same arguments (so a copy
//! started with Windows comes back in the tray). Nothing about the user is sent.
//!
//! People's own data is never part of an update: edits, favourites and export history are in
//! %APPDATA%\Backtrack\store.json and settings in settings.json, outside the install folder, and the installer runs
//! in update mode, which replaces the program files without running the uninstaller.

use crate::{util, AppState};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
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
        // Let an export in progress finish (up to 10 minutes) rather than cut it short.
        for _ in 0..600 {
            if !st.job.running.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
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

/// After an update restarted the app, say so once.
pub fn announce_if_updated(app: &AppHandle) {
    let Ok(v) = std::fs::read_to_string(marker()) else { return };
    let _ = std::fs::remove_file(marker());
    let v = v.trim();
    if v == app.package_info().version.to_string() {
        crate::rec_app::toast(app, "Backtrack updated", &format!("Now on version {v}"), "ok");
    }
}

#[cfg(test)]
mod tests {
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
