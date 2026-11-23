# Qeezy Launcher

A private Minecraft launcher for a small group of friends, with **Qeezy Client** built in.

> **Qeezy Launcher is a modified version of [OneLauncher / OneClient](https://github.com/Polyfrost/OneLauncher) by Polyfrost and Contributors**, based on release `oneclient-2.12.1`.
> It is licensed under the **GNU General Public License v3.0 only**, the same as the original: see [LICENSE](./LICENSE) and [ATTRIBUTION.md](./ATTRIBUTION.md).
> Not affiliated with or endorsed by Polyfrost, Mojang or Microsoft. The original README is kept in [README.upstream.md](./README.upstream.md).

## What's different from OneClient

- **Name and look:** Qeezy name, icons, logo, colours and home background. Polyfrost's branding (all rights reserved) has been removed. The "Design © Polyfrost" notice stays as their copyright credit.
- **Own data folder** (`Qeezy/QeezyLauncher`), so it never touches an existing OneClient install.
- **Qeezy Client built in:** every Fabric 1.21.11 instance gets the bundled Qeezy Client mod (`packages/oneclient_core/assets/qeezy-client.jar`).
- **Play via official launcher:** until Mojang approves Qeezy Launcher's own Microsoft sign-in, Play prepares the instance here, then adds a "Qeezy · …" profile to the official Minecraft Launcher and opens it (`packages/oneclient_core/src/game/official.rs`). The official launcher handles the account and game ownership.
- **No Polyfrost accounts or keys:** no crash reports to Polyfrost's Sentry, no Discord rich presence app, no CurseForge key, no Polyfrost Microsoft app ID, and no self-updates from Polyfrost's releases. Each of these can be switched back on with our own keys at build time (see below).
- Version metadata and bundles are still read from Polyfrost's public metadata server.

## Build-time settings

| Variable | What it does |
| --- | --- |
| `QEEZY_MSA_CLIENT_ID` | Our Azure app ID. Set it once Mojang approves it, and Microsoft sign-in works inside the launcher. |
| `QEEZY_UPDATER_ENDPOINT`, `QEEZY_UPDATER_PUBKEY` | Self-update feed and minisign public key. Leave both empty to turn updates off. |
| `QEEZY_SENTRY_DSN`, `QEEZY_DISCORD_CLIENT_ID`, `QEEZY_CURSEFORGE_API_KEY` | Optional: our own crash reports, Discord presence and CurseForge access. |

## Building

- **Windows installer:** GitHub → Actions → **Qeezy Launcher (Windows)** → Run workflow. The installer appears under the run's artifacts. Pushing a tag like `v1.0.0` also attaches it to a release.
- **Locally:** `cargo build --release --bin oneclient_app` (Rust 1.97+). Upstream's original workflows are kept in `.github/workflows-upstream/` for reference.

## Source

Under the GPL, anyone you give Qeezy Launcher to can ask for this source code. Keep this repository and share it with them.
