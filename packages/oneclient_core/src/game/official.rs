//! Qeezy Launcher additions (not part of upstream OneLauncher):
//! - every Fabric 1.21.11 instance gets the bundled Qeezy Client mod
//! - "Play via official launcher": prepares an instance, then adds it as a profile in the
//!   official Minecraft Launcher and opens it. Used until Mojang approves Qeezy Launcher's
//!   own Microsoft sign-in; the official launcher handles the account and game ownership.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use oneclient_auth::{AccountKind, MinecraftAccount};
use oneclient_common::domain::GameLoader;
use oneclient_common::paths;
use oneclient_content::bundles::BUNDLE_APPLY_BUDGET;
use serde_json::{Value, json};

use crate::LauncherResult;
use crate::clusters::Cluster;
use crate::game::GameError;
use crate::state::LauncherState;

/// Qeezy Client is built for this Minecraft version only.
pub const QEEZY_CLIENT_MC_VERSION: &str = "1.21.11";
const QEEZY_CLIENT_JAR: &[u8] = include_bytes!("../../assets/qeezy-client.jar");
const QEEZY_CLIENT_FILE: &str = "qeezy-client.jar";
const QEEZY_ICON: &[u8] = include_bytes!("../../assets/qeezy-icon.png");
const PROFILE_PREFIX: &str = "qeezy-";

/// Puts the bundled Qeezy Client jar into a Fabric 1.21.11 instance's mods folder,
/// replacing any older copy. Other instances are left alone.
pub async fn ensure_qeezy_client(cluster: &Cluster, mods_dir: &Path) {
    let mc = oneclient_common::version::normalize_mc_version_input(&cluster.mc_version);
    if cluster.mc_loader != GameLoader::Fabric || mc != QEEZY_CLIENT_MC_VERSION {
        return;
    }
    if let Err(err) = write_qeezy_client(mods_dir).await {
        tracing::warn!(cluster_id = cluster.id, error = %err, "failed to add Qeezy Client to the instance");
    }
}

async fn write_qeezy_client(mods_dir: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(mods_dir).await?;
    if let Ok(mut entries) = tokio::fs::read_dir(mods_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("qeezy-client") && name.ends_with(".jar") && name != QEEZY_CLIENT_FILE {
                tokio::fs::remove_file(entry.path()).await.ok();
            }
        }
    }
    let target = mods_dir.join(QEEZY_CLIENT_FILE);
    let same = tokio::fs::read(&target).await.is_ok_and(|bytes| bytes == QEEZY_CLIENT_JAR);
    if !same {
        let tmp = mods_dir.join(".qeezy-client.jar.part");
        tokio::fs::write(&tmp, QEEZY_CLIENT_JAR).await?;
        tokio::fs::rename(&tmp, &target).await?;
    }
    Ok(())
}

/// Qeezy Client's mod pack downloads mods by itself. If the same mod is also provided by another
/// jar in the instance (a launcher bundle, or one the player added), Fabric refuses to start, so the
/// copy Qeezy Client downloaded is removed and forgotten. Mods it didn't download are never touched.
pub async fn resolve_qeezy_pack_duplicates(game_dir: &Path, mods_dir: &Path) {
    let state_file = game_dir.join("config").join("qeezyclient").join("pack-state.json");
    let Ok(text) = tokio::fs::read_to_string(&state_file).await else {
        return;
    };
    let Ok(mut state) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let Some(installed) = state.get("installed").and_then(Value::as_object).cloned() else {
        return;
    };
    // file name -> slug, for the mods Qeezy Client downloaded
    let ours: std::collections::HashMap<String, String> = installed
        .iter()
        .filter_map(|(slug, rec)| Some((rec.get("file")?.as_str()?.to_string(), slug.clone())))
        .collect();
    if ours.is_empty() {
        return;
    }

    let Ok(mut entries) = tokio::fs::read_dir(mods_dir).await else {
        return;
    };
    let mut our_ids: Vec<(String, String, PathBuf)> = Vec::new(); // (mod id, slug, path)
    let mut other_ids = std::collections::HashSet::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".jar") || name == QEEZY_CLIENT_FILE {
            continue;
        }
        let path = entry.path();
        let Some(id) = oneclient_content::packages::local_manifest::read_jar_mod_id(&path).await else {
            continue;
        };
        match ours.get(&name) {
            Some(slug) => our_ids.push((id.id, slug.clone(), path)),
            None => {
                other_ids.insert(id.id);
            }
        }
    }

    let mut changed = false;
    for (id, slug, path) in our_ids {
        if other_ids.contains(&id) {
            tracing::info!(mod_id = %id, file = %path.display(), "removing Qeezy pack copy that another jar already provides");
            if tokio::fs::remove_file(&path).await.is_ok() {
                if let Some(map) = state.get_mut("installed").and_then(Value::as_object_mut) {
                    map.remove(&slug);
                }
                changed = true;
            }
        }
    }
    if changed && let Ok(text) = serde_json::to_string_pretty(&state) {
        tokio::fs::write(&state_file, text).await.ok();
    }
}

/// Result of trying to reuse the account the official Minecraft Launcher already signed in.
pub enum OfficialAccount {
    /// A valid token was found — launch the game directly with it.
    Ready(Box<MinecraftAccount>),
    /// An account exists but its token has expired — open the official launcher once to refresh.
    Stale,
    /// No usable account was found (never logged in to the official launcher).
    Missing,
}

/// Reads the token the official Minecraft Launcher saved for the user's own account, so Qeezy can
/// launch the game straight away without opening the official launcher window. This is the user's
/// own account and token on their own machine — nothing is authenticated or impersonated here; the
/// official launcher did the real sign-in. The token is short-lived, so the user reopens the
/// official launcher now and then to refresh it.
pub fn read_official_minecraft_account() -> OfficialAccount {
    let mut files = Vec::new();
    if let Some(dir) = official_minecraft_dir() {
        files.push(dir.join("launcher_accounts.json"));
        files.push(dir.join("launcher_accounts_microsoft_store.json"));
    }
    // Microsoft Store / Xbox app packaged launcher keeps its own .minecraft
    #[cfg(target_os = "windows")]
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        let packaged = PathBuf::from(base)
            .join("Packages")
            .join("Microsoft.4297127D64EC6_8wekyb3d8bbwe")
            .join("LocalCache")
            .join("Roaming")
            .join(".minecraft");
        files.push(packaged.join("launcher_accounts.json"));
    }

    let mut saw_stale = false;
    for file in files {
        match account_from_file(&file) {
            Some(Ok(account)) => return OfficialAccount::Ready(Box::new(account)),
            Some(Err(())) => saw_stale = true, // found an account but the token was expired
            None => {}
        }
    }
    if saw_stale { OfficialAccount::Stale } else { OfficialAccount::Missing }
}

/// None = no account in this file; Some(Err) = account present but token expired; Some(Ok) = usable.
fn account_from_file(file: &Path) -> Option<Result<MinecraftAccount, ()>> {
    let text = std::fs::read_to_string(file).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    let accounts = root.get("accounts")?.as_object()?;
    if accounts.is_empty() {
        return None;
    }
    let active = root.get("activeAccountLocalId").and_then(Value::as_str);

    // Prefer the active account, else the first one that has a Minecraft profile.
    let ordered = active
        .and_then(|id| accounts.get(id).map(|a| (id, a)))
        .into_iter()
        .chain(accounts.iter().map(|(k, v)| (k.as_str(), v)));

    for (_, acc) in ordered {
        let profile = match acc.get("minecraftProfile").and_then(Value::as_object) {
            Some(p) => p,
            None => continue,
        };
        let token = match acc.get("accessToken").and_then(Value::as_str) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => continue,
        };
        let name = profile.get("name").and_then(Value::as_str).unwrap_or("Player").to_string();
        let id = profile
            .get("id")
            .and_then(Value::as_str)
            .and_then(|s| uuid::Uuid::try_parse(s).ok())
            .unwrap_or_else(uuid::Uuid::nil);

        let expires = acc
            .get("accessTokenExpiresAt")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        // If we know the expiry and it has already passed, the token is stale.
        if let Some(exp) = expires
            && exp <= Utc::now()
        {
            return Some(Err(()));
        }

        return Some(Ok(MinecraftAccount {
            id,
            username: name,
            access_token: token,
            refresh_token: String::new(),
            expires: expires.unwrap_or_else(|| Utc::now() + chrono::Duration::hours(12)),
            kind: AccountKind::Microsoft,
            signed_out: false,
        }));
    }
    None
}

#[derive(Debug, Clone)]
pub struct OfficialHandoff {
    /// Name of the profile as it appears in the official launcher.
    pub profile_name: String,
    /// Whether the official launcher could be started for the user.
    pub opened: bool,
}

/// The official launcher's folder (.minecraft).
pub fn official_minecraft_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join(".minecraft"))
    }
    #[cfg(target_os = "macos")]
    {
        directories::BaseDirs::new().map(|d| d.home_dir().join("Library/Application Support/minecraft"))
    }
    #[cfg(target_os = "linux")]
    {
        directories::BaseDirs::new().map(|d| d.home_dir().join(".minecraft"))
    }
}

fn handoff_error(msg: impl Into<String>) -> crate::LauncherError {
    GameError::OfficialLauncher(msg.into()).into()
}

/// Prepares the instance exactly like a normal launch would (game files, loader, mods,
/// bundles, Qeezy Client), then hands it to the official Minecraft Launcher.
#[tracing::instrument(skip(state))]
pub async fn hand_off_to_official_launcher(
    state: &Arc<LauncherState>,
    cluster_id: i64,
) -> LauncherResult<OfficialHandoff> {
    let mc_dir = official_minecraft_dir()
        .ok_or_else(|| handoff_error("Couldn't find the Minecraft folder on this computer"))?;

    // Same as a normal launch: bundles can pin a newer Java (e.g. C2ME needs Java 25),
    // and the official launcher would otherwise run the game on its own Java 21.
    if let Err(err) = crate::clusters::apply_bundle_java_override(state, cluster_id, true, true, None).await {
        tracing::warn!(cluster_id, error = %err, "failed to apply bundle java override");
    }

    let existing = state.clusters.get(cluster_id).await?;
    let cluster = if existing.stage == crate::ClusterStage::Ready {
        existing
    } else {
        crate::clusters::prepare_cluster_locked(state, cluster_id, false, true, false, None).await?
    };
    if cluster.uses_bundles()
        && let Err(err) = oneclient_content::bundles::install_cluster_bundles(
            cluster_id,
            state.bundles.as_ref(),
            None,
            &state.services.content(),
            Some(Instant::now() + BUNDLE_APPLY_BUDGET),
        )
        .await
    {
        tracing::warn!(cluster_id, error = %err, "failed to install bundle content");
    }
    let cluster = match crate::clusters::auto_update_loader(state, &cluster).await {
        Ok(updated) => updated.unwrap_or(cluster),
        Err(err) => {
            tracing::warn!(cluster_id, error = %err, "failed to update the loader");
            cluster
        }
    };

    let global = state.settings.read().global_game_settings.clone();
    let profile = state.clusters.resolve_settings(&global, &cluster).await?;
    let mc_version = oneclient_common::version::normalize_mc_version_input(&cluster.mc_version);

    let (version_id, loader_version_id, version_info) = {
        let mut metadata = state.metadata.lock().await;
        let (version, _index, _updated) =
            crate::game::resolve_minecraft_version(&mut metadata, &state.services.mc(), &mc_version).await?;
        let loader_version = crate::game::get_loader_version(
            &mut metadata,
            &state.services.mc(),
            &mc_version,
            cluster.mc_loader,
            cluster.mc_loader_version.as_deref(),
        )
        .await?;
        let version_info = crate::game::download_version_info(
            &state.services.mc(),
            None,
            &version,
            cluster.mc_loader,
            loader_version.as_ref(),
            false,
        )
        .await?;
        (version.id.clone(), loader_version.map(|lv| lv.id), version_info)
    };

    // The Java this instance needs, picked exactly like a normal launch
    let native_ok = crate::game::natives::native_java_ok(&version_info);
    let pinned = state.java.runtime_for_profile(profile.java_path.as_deref()).await?;
    let java = match pinned {
        Some(runtime) if !oneclient_java::wrong_arch_for_game(&runtime.os_arch, native_ok) => runtime,
        pinned => {
            let major = match &pinned {
                Some(runtime) => runtime.major,
                None => version_info
                    .java_version
                    .as_ref()
                    .map(|v| v.major_version)
                    .ok_or(GameError::MissingJavaVersion)?,
            };
            state.java.prepare(major, native_ok, true, true, None).await?
        }
    };

    // Mods and the rest of the instance's content, same as a normal launch.
    let game_dir = cluster.game_dir()?;
    tokio::fs::create_dir_all(&game_dir).await.ok();
    let custom_args = profile.launch_args.clone().unwrap_or_default();
    let mods_in_cluster =
        crate::game::uses_cluster_mods_folder(cluster.mc_loader, loader_version_id.as_deref(), &custom_args);
    crate::game::materialize_content(
        &state.services,
        &cluster,
        &game_dir,
        mods_in_cluster,
        profile.copy_mod_files.unwrap_or(false),
    )
    .await?;
    let mods_dir = if mods_in_cluster {
        paths::cluster_mods_dir(&cluster.folder_name)?
    } else {
        game_dir.join("mods")
    };
    ensure_qeezy_client(&cluster, &mods_dir).await;
    resolve_qeezy_pack_duplicates(&game_dir, &mods_dir).await;

    // The version the official launcher should start.
    let last_version_id = match (cluster.mc_loader, loader_version_id.as_deref()) {
        (GameLoader::Vanilla, _) => version_id.clone(),
        (GameLoader::Fabric | GameLoader::Quilt, Some(loader)) => {
            let url = if cluster.mc_loader == GameLoader::Fabric {
                format!("https://meta.fabricmc.net/v2/versions/loader/{version_id}/{loader}/profile/json")
            } else {
                format!("https://meta.quiltmc.org/v3/versions/loader/{version_id}/{loader}/profile/json")
            };
            let body = state
                .services
                .requester
                .http()
                .get(&url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|err| handoff_error(format!("Couldn't get the loader profile: {err}")))?
                .text()
                .await
                .map_err(|err| handoff_error(format!("Couldn't read the loader profile: {err}")))?;
            let profile_json: Value = serde_json::from_str(&body)?;
            let id = profile_json["id"]
                .as_str()
                .ok_or_else(|| handoff_error("The loader profile has no id"))?
                .to_string();
            let dir = mc_dir.join("versions").join(&id);
            tokio::fs::create_dir_all(&dir).await.map_err(|e| handoff_error(e.to_string()))?;
            tokio::fs::write(dir.join(format!("{id}.json")), body)
                .await
                .map_err(|e| handoff_error(e.to_string()))?;
            id
        }
        (loader, _) => {
            return Err(handoff_error(format!(
                "{loader} instances can't open in the official launcher yet. Use a Fabric, Quilt or vanilla instance."
            )));
        }
    };

    // JVM args: memory + the instance's mods folder + the user's own args.
    let mem = profile.mem_max.unwrap_or_else(oneclient_common::default_mem_max);
    let mut java_args = format!("-Xmx{mem}M");
    if let Some(arg) = crate::game::mods_folder_argument(
        cluster.mc_loader,
        loader_version_id.as_deref(),
        &custom_args,
        &mods_dir,
    ) {
        // The official launcher splits JVM args on spaces and passes quotes through as-is,
        // which turns a quoted arg into the "main class" and kills the game with exit code 1.
        // Only quote when the path really has a space.
        if arg.contains(char::is_whitespace) {
            java_args.push_str(&format!(" \"{arg}\""));
        } else {
            java_args.push(' ');
            java_args.push_str(&arg);
        }
    }
    if !custom_args.trim().is_empty() {
        java_args.push(' ');
        java_args.push_str(custom_args.trim());
    }

    let profile_name = format!("Qeezy · {}", cluster.name);
    write_launcher_profile(&mc_dir, cluster.id, &profile_name, &last_version_id, &game_dir, &java_args, Some(&java.absolute_path)).await?;

    let opened = open_official_launcher().await;
    tracing::info!(cluster_id, opened, version = %last_version_id, "handed instance to the official launcher");
    Ok(OfficialHandoff { profile_name, opened })
}

async fn write_launcher_profile(
    mc_dir: &Path,
    cluster_id: i64,
    name: &str,
    version: &str,
    game_dir: &Path,
    java_args: &str,
    java_exe: Option<&str>,
) -> LauncherResult<()> {
    let file = mc_dir.join("launcher_profiles.json");
    tokio::fs::create_dir_all(mc_dir).await.map_err(|e| handoff_error(e.to_string()))?;
    let mut root: Value = match tokio::fs::read_to_string(&file).await {
        Ok(text) => {
            // keep a copy in case anything goes wrong
            tokio::fs::write(mc_dir.join("launcher_profiles.qeezy-backup.json"), &text).await.ok();
            serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
        }
        Err(_) => json!({}),
    };
    if !root.is_object() {
        root = json!({});
    }
    if !root["profiles"].is_object() {
        root["profiles"] = json!({});
    }
    if root.get("version").is_none() {
        root["version"] = json!(3);
    }
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let key = format!("{PROFILE_PREFIX}{cluster_id}");
    let created = root["profiles"][&key]["created"].as_str().map(str::to_string).unwrap_or_else(|| now.clone());
    let icon = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(QEEZY_ICON)
    );
    root["profiles"][&key] = json!({
        "name": name,
        "type": "custom",
        "created": created,
        "lastUsed": now,
        "lastVersionId": version,
        "gameDir": game_dir.display().to_string(),
        "javaArgs": java_args,
        "icon": icon,
    });
    if let Some(exe) = java_exe {
        root["profiles"][&key]["javaDir"] = json!(exe);
    }
    let text = serde_json::to_string_pretty(&root)?;
    let tmp = mc_dir.join("launcher_profiles.json.qeezy-tmp");
    tokio::fs::write(&tmp, text).await.map_err(|e| handoff_error(e.to_string()))?;
    tokio::fs::rename(&tmp, &file).await.map_err(|e| handoff_error(e.to_string()))?;
    Ok(())
}

/// Starts whichever official launcher is installed. Returns false if none was found.
async fn open_official_launcher() -> bool {
    #[cfg(target_os = "windows")]
    {
        let mut candidates: Vec<PathBuf> = Vec::new();
        for var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(base) = std::env::var_os(var) {
                candidates.push(PathBuf::from(base).join("Minecraft Launcher").join("MinecraftLauncher.exe"));
            }
        }
        candidates.push(PathBuf::from(r"C:\XboxGames\Minecraft Launcher\Content\Minecraft.exe"));
        for exe in candidates {
            if exe.is_file() && std::process::Command::new(&exe).spawn().is_ok() {
                return true;
            }
        }
        // Microsoft Store / Xbox app version
        let store_installed = std::env::var_os("LOCALAPPDATA").is_some_and(|base| {
            PathBuf::from(base).join("Packages").join("Microsoft.4297127D64EC6_8wekyb3d8bbwe").is_dir()
        });
        if store_installed {
            return std::process::Command::new("explorer.exe")
                .arg(r"shell:AppsFolder\Microsoft.4297127D64EC6_8wekyb3d8bbwe!Minecraft")
                .spawn()
                .is_ok();
        }
        false
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").args(["-a", "Minecraft"]).status().is_ok_and(|s| s.success())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("minecraft-launcher").spawn().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qeezy-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn qeezy_client_replaces_old_copies_and_keeps_other_mods() {
        let mods = temp_dir("mods");
        std::fs::write(mods.join("qeezy-client-1.0.0.jar"), b"old").unwrap();
        std::fs::write(mods.join("sodium.jar"), b"sodium").unwrap();
        write_qeezy_client(&mods).await.unwrap();
        assert!(!mods.join("qeezy-client-1.0.0.jar").exists(), "old Qeezy Client should be removed");
        assert!(mods.join("sodium.jar").exists(), "other mods must stay");
        assert_eq!(std::fs::read(mods.join(QEEZY_CLIENT_FILE)).unwrap(), QEEZY_CLIENT_JAR);
        // running again changes nothing
        write_qeezy_client(&mods).await.unwrap();
        assert_eq!(std::fs::read_dir(&mods).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn launcher_profile_is_added_without_touching_other_profiles() {
        let mc = temp_dir("dotminecraft");
        std::fs::write(
            mc.join("launcher_profiles.json"),
            r#"{"profiles":{"abc":{"name":"Latest release","type":"latest-release"}},"settings":{"locale":"en-us"},"version":3}"#,
        )
        .unwrap();
        let game_dir = mc.join("instances").join("qeezy");
        write_launcher_profile(&mc, 7, "Qeezy · Test", "fabric-loader-0.18.4-1.21.11", &game_dir, "-Xmx4096M", None)
            .await
            .unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(mc.join("launcher_profiles.json")).unwrap()).unwrap();
        assert_eq!(root["profiles"]["abc"]["name"], "Latest release");
        assert_eq!(root["settings"]["locale"], "en-us");
        let p = &root["profiles"]["qeezy-7"];
        assert_eq!(p["name"], "Qeezy · Test");
        assert_eq!(p["type"], "custom");
        assert_eq!(p["lastVersionId"], "fabric-loader-0.18.4-1.21.11");
        assert_eq!(p["gameDir"], game_dir.display().to_string());
        assert!(p["icon"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert!(mc.join("launcher_profiles.qeezy-backup.json").exists());

        // a second hand-off keeps the original "created" date
        let created = p["created"].clone();
        write_launcher_profile(&mc, 7, "Qeezy · Test", "fabric-loader-0.18.4-1.21.11", &game_dir, "-Xmx4096M", None)
            .await
            .unwrap();
        let again: Value = serde_json::from_str(&std::fs::read_to_string(mc.join("launcher_profiles.json")).unwrap()).unwrap();
        assert_eq!(again["profiles"]["qeezy-7"]["created"], created);
        assert_eq!(again["profiles"].as_object().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn launcher_profile_file_is_created_when_missing() {
        let mc = temp_dir("fresh");
        write_launcher_profile(&mc, 1, "Qeezy · Fresh", "1.21.11", &mc.join("g"), "-Xmx2048M", None).await.unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(mc.join("launcher_profiles.json")).unwrap()).unwrap();
        assert_eq!(root["version"], 3);
        assert_eq!(root["profiles"]["qeezy-1"]["lastVersionId"], "1.21.11");
    }
}

#[cfg(test)]
mod duplicate_tests {
    use super::*;

    /// Uses real mod jars from a Qeezy Client test run when they're around.
    #[tokio::test]
    async fn pack_copy_is_removed_only_when_another_jar_provides_the_same_mod() {
        let src = PathBuf::from("/home/claude/qeezy-client/run/mods");
        let sodium = src.join("sodium-fabric-0.8.14+mc1.21.11.jar");
        let lithium = src.join("lithium-fabric-0.21.4+mc1.21.11.jar");
        if !sodium.exists() || !lithium.exists() {
            return;
        }
        let game = std::env::temp_dir().join(format!("qeezy-dupes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&game);
        let mods = game.join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::create_dir_all(game.join("config/qeezyclient")).unwrap();
        // Qeezy pack downloaded sodium + lithium; a launcher bundle also put in sodium under another name
        std::fs::copy(&sodium, mods.join("sodium-fabric-0.8.14+mc1.21.11.jar")).unwrap();
        std::fs::copy(&lithium, mods.join("lithium-fabric-0.21.4+mc1.21.11.jar")).unwrap();
        std::fs::copy(&sodium, mods.join("bundle-sodium.jar")).unwrap();
        std::fs::write(
            game.join("config/qeezyclient/pack-state.json"),
            r#"{"installed":{"sodium":{"versionId":"a","file":"sodium-fabric-0.8.14+mc1.21.11.jar"},"lithium":{"versionId":"b","file":"lithium-fabric-0.21.4+mc1.21.11.jar"}},"swaps":[]}"#,
        )
        .unwrap();

        resolve_qeezy_pack_duplicates(&game, &mods).await;

        assert!(!mods.join("sodium-fabric-0.8.14+mc1.21.11.jar").exists(), "Qeezy's duplicate copy goes");
        assert!(mods.join("bundle-sodium.jar").exists(), "the other copy stays");
        assert!(mods.join("lithium-fabric-0.21.4+mc1.21.11.jar").exists(), "unique mods stay");
        let state: Value = serde_json::from_str(&std::fs::read_to_string(game.join("config/qeezyclient/pack-state.json")).unwrap()).unwrap();
        assert!(state["installed"].get("sodium").is_none());
        assert!(state["installed"].get("lithium").is_some());
    }
}

#[cfg(test)]
mod official_account_tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("qeezy-acct-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn rand_suffix() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    #[test]
    fn reads_the_active_account_with_a_valid_token() {
        let dir = tmp();
        let future = (Utc::now() + chrono::Duration::hours(10)).to_rfc3339();
        std::fs::write(
            dir.join("launcher_accounts.json"),
            format!(
                r#"{{"accounts":{{
                    "aaa":{{"accessToken":"TOKEN_A","accessTokenExpiresAt":"{future}","minecraftProfile":{{"id":"0123456789abcdef0123456789abcdef","name":"Qeezy"}}}},
                    "bbb":{{"accessToken":"TOKEN_B","minecraftProfile":{{"id":"ffffffffffffffffffffffffffffffff","name":"Other"}}}}
                }},"activeAccountLocalId":"bbb"}}"#
            ),
        )
        .unwrap();
        match account_from_file(&dir.join("launcher_accounts.json")) {
            Some(Ok(acc)) => {
                assert_eq!(acc.username, "Other"); // the active one
                assert_eq!(acc.access_token, "TOKEN_B");
                assert_eq!(acc.kind, AccountKind::Microsoft);
            }
            other => panic!("expected a ready account, got {:?}", other.map(|r| r.map(|a| a.username))),
        }
    }

    #[test]
    fn an_expired_token_is_reported_stale() {
        let dir = tmp();
        let past = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        std::fs::write(
            dir.join("launcher_accounts.json"),
            format!(
                r#"{{"accounts":{{"aaa":{{"accessToken":"OLD","accessTokenExpiresAt":"{past}","minecraftProfile":{{"id":"0123456789abcdef0123456789abcdef","name":"Qeezy"}}}}}},"activeAccountLocalId":"aaa"}}"#
            ),
        )
        .unwrap();
        assert!(matches!(account_from_file(&dir.join("launcher_accounts.json")), Some(Err(()))));
    }

    #[test]
    fn no_file_and_empty_accounts_are_missing() {
        let dir = tmp();
        assert!(account_from_file(&dir.join("nope.json")).is_none());
        std::fs::write(dir.join("empty.json"), r#"{"accounts":{}}"#).unwrap();
        assert!(account_from_file(&dir.join("empty.json")).is_none());
    }
}
