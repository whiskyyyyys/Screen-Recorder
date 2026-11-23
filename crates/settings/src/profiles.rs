use crate::models::AppSettings;
use directories::ProjectDirs;
use obs_core::{ObsError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRootState {
    pub active_profile: String,
    pub active_collection: String,
    pub studio_mode: bool,
    pub dock_layout_json: Option<String>,
    pub window_maximized: bool,
}

impl Default for AppRootState {
    fn default() -> Self {
        Self {
            active_profile: "Default".to_string(),
            active_collection: "Default".to_string(),
            studio_mode: false,
            dock_layout_json: None,
            window_maximized: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProfileManager {
    config_root: PathBuf,
}

impl ProfileManager {
    pub fn discover() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));

        if let Some(dir) = &exe_dir {
            if dir.join("portable_mode.txt").exists() || dir.join("portable_mode").exists() {
                return Self::with_root(dir.join("config"));
            }
        }

        let root = ProjectDirs::from("org", "OxideStudio", "OxideStudio")
            .map(|p| p.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("config"));
        Self::with_root(root)
    }

    pub fn with_root(config_root: PathBuf) -> Self {
        let _ = fs::create_dir_all(config_root.join("profiles"));
        let _ = fs::create_dir_all(config_root.join("collections"));
        let _ = fs::create_dir_all(config_root.join("plugins"));
        Self { config_root }
    }

    pub fn config_root(&self) -> &Path {
        &self.config_root
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.config_root.join("profiles")
    }

    pub fn collections_dir(&self) -> PathBuf {
        self.config_root.join("collections")
    }

    pub fn user_plugins_dir(&self) -> PathBuf {
        self.config_root.join("plugins")
    }

    pub fn load_root_state(&self) -> AppRootState {
        let p = self.config_root.join("state.json");
        if let Ok(text) = fs::read_to_string(p) {
            if let Ok(state) = serde_json::from_str::<AppRootState>(&text) {
                return state;
            }
        }
        AppRootState::default()
    }

    pub fn save_root_state(&self, state: &AppRootState) -> Result<()> {
        let p = self.config_root.join("state.json");
        fs::create_dir_all(&self.config_root)?;
        fs::write(p, serde_json::to_string_pretty(state)?)?;
        Ok(())
    }

    pub fn list_profiles(&self) -> Vec<String> {
        let mut names = list_json_stems(&self.profiles_dir());
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }

    pub fn load_profile(&self, name: &str) -> Result<AppSettings> {
        let path = self.profile_path(name);
        if !path.exists() {
            let defaults = AppSettings::default();
            self.save_profile(name, &defaults)?;
            return Ok(defaults);
        }
        let raw = fs::read_to_string(&path)?;
        let settings = serde_json::from_str::<AppSettings>(&raw)?;
        Ok(settings)
    }

    pub fn save_profile(&self, name: &str, settings: &AppSettings) -> Result<()> {
        fs::create_dir_all(self.profiles_dir())?;
        let path = self.profile_path(name);
        fs::write(path, serde_json::to_string_pretty(settings)?)?;
        Ok(())
    }

    pub fn create_profile(&self, name: &str, base_settings: Option<&AppSettings>) -> Result<()> {
        let safe = sanitize_name(name)?;
        let path = self.profile_path(&safe);
        if path.exists() {
            return Err(ObsError::AlreadyExists(format!("Profile '{safe}'")));
        }
        let data = base_settings.cloned().unwrap_or_default();
        self.save_profile(&safe, &data)
    }

    pub fn rename_profile(&self, old_name: &str, new_name: &str) -> Result<()> {
        let safe_new = sanitize_name(new_name)?;
        let old_p = self.profile_path(old_name);
        let new_p = self.profile_path(&safe_new);
        if new_p.exists() {
            return Err(ObsError::AlreadyExists(format!("Profile '{safe_new}'")));
        }
        if old_p.exists() {
            fs::rename(old_p, new_p)?;
        } else {
            self.save_profile(&safe_new, &AppSettings::default())?;
        }
        Ok(())
    }

    pub fn delete_profile(&self, name: &str) -> Result<()> {
        let profiles = self.list_profiles();
        if profiles.len() <= 1 {
            return Err(ObsError::InvalidArgument(
                "Cannot delete the last remaining profile".to_string(),
            ));
        }
        let path = self.profile_path(name);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn list_collections(&self) -> Vec<String> {
        let mut names = list_json_stems(&self.collections_dir());
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }

    pub fn collection_path(&self, name: &str) -> PathBuf {
        let safe = name.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
        self.collections_dir().join(format!("{safe}.json"))
    }

    pub fn profile_path(&self, name: &str) -> PathBuf {
        let safe = name.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
        self.profiles_dir().join(format!("{safe}.json"))
    }
}

fn sanitize_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ObsError::InvalidArgument(
            "Name cannot be empty".to_string(),
        ));
    }
    Ok(trimmed.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_"))
}

fn list_json_stems(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.push(stem.to_string());
                }
            }
        }
    }
    out.sort();
    out
}
