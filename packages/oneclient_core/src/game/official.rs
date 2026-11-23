//! Qeezy Launcher additions (not part of upstream OneLauncher):
//! - every Fabric 1.21.11 instance gets the bundled Qeezy Client mod
//! - "Play via official launcher": prepares an instance, then adds it as a profile in the
//!   official Minecraft Launcher and opens it. Used until Mojang approves Qeezy Launcher's
//!   own Microsoft sign-in; the official launcher handles the account and game ownership.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine as _;
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

    let (version_id, loader_version_id) = {
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
        (version.id.clone(), loader_version.map(|lv| lv.id))
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
        java_args.push_str(&format!(" \"{arg}\""));
    }
    if !custom_args.trim().is_empty() {
        java_args.push(' ');
        java_args.push_str(custom_args.trim());
    }

    let profile_name = format!("Qeezy · {}", cluster.name);
    write_launcher_profile(&mc_dir, cluster.id, &profile_name, &last_version_id, &game_dir, &java_args).await?;

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
