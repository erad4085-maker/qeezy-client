pub const WINDOW_APP_ID: &str = "qeezy_launcher";
pub const WINDOW_TITLE: &str = "Qeezy Launcher";

/// Self-updates come from Qeezy's own GitHub releases, signed with our own key (set at build time).
/// Both empty = the updater stays off.
pub const UPDATER_PUBKEY: &str = match option_env!("QEEZY_UPDATER_PUBKEY") {
    Some(key) => key,
    None => "",
};
pub const UPDATER_ENDPOINT: &str = match option_env!("QEEZY_UPDATER_ENDPOINT") {
    Some(url) => url,
    None => "",
};
pub const RELEASES_URL: &str = match option_env!("QEEZY_RELEASES_URL") {
    Some(url) => url,
    None => "https://github.com/",
};
