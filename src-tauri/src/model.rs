use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AppId {
    Prowlarr,
    Sonarr,
    Radarr,
    FlareSolverr,
    QBittorrent,
    Jellyfin,
}

impl AppId {
    pub const ALL: [AppId; 6] = [
        AppId::Prowlarr,
        AppId::Sonarr,
        AppId::Radarr,
        AppId::FlareSolverr,
        AppId::QBittorrent,
        AppId::Jellyfin,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AppId::Prowlarr => "Prowlarr",
            AppId::Sonarr => "Sonarr",
            AppId::Radarr => "Radarr",
            AppId::FlareSolverr => "FlareSolverr",
            AppId::QBittorrent => "qBittorrent",
            AppId::Jellyfin => "Jellyfin",
        }
    }

    /// Prowlarr, Sonarr and Radarr: the apps with an `*arr` API and a config.xml.
    pub fn is_arr(self) -> bool {
        matches!(self, AppId::Prowlarr | AppId::Sonarr | AppId::Radarr)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    pub id: AppId,
    pub base_url: String,
    /// Read from the app's own config file on every discovery. Never saved.
    #[serde(skip)]
    pub api_key: Option<String>,
    pub exe: Option<PathBuf>,
    pub service: Option<String>,
    pub managed: bool,
    /// Why discovery could not fully set this app up, in plain words.
    pub problem: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Amber,
    Red,
    Unknown,
}

impl Level {
    pub fn rank(self) -> u8 {
        match self {
            Level::Ok => 0,
            Level::Unknown => 1,
            Level::Amber => 2,
            Level::Red => 3,
        }
    }
}
