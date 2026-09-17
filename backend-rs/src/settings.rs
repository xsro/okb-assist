//! 全局 Settings 访问器。

use std::sync::{Arc, OnceLock};

use crate::config::Settings;
use crate::config_manager::ConfigManager;

static GLOBAL_SETTINGS: OnceLock<Arc<Settings>> = OnceLock::new();

/// 初始化全局 Settings（进程启动时调用一次）
pub fn init_settings(settings: Arc<Settings>) {
    let _ = GLOBAL_SETTINGS.set(settings);
}

/// 获取全局 Settings
pub fn get_settings() -> Arc<Settings> {
    GLOBAL_SETTINGS
        .get_or_init(|| Arc::new(Settings::new(Arc::new(ConfigManager::new()))))
        .clone()
}
