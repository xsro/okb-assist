//! 从 system.json 推导文档相关路径。
//!
//! 支持以下变量替换（通过 ConfigManager::substitute_path_variables）：
//! - `{id}` — 文档 ID
//! - `{env:VAR_NAME}` — 环境变量 VAR_NAME 的值

use std::path::PathBuf;

use crate::config::Settings;

/// 解析路径模板中的变量。
///
/// 支持的变量：
/// - `{id}` — 文档 ID
/// - `{env:VAR_NAME}` — 环境变量 VAR_NAME 的值
pub fn resolve_template(template: &str, doc_id: i64, _settings: &Settings) -> String {
    crate::config_manager::ConfigManager::substitute_path_variables(template, doc_id)
}

pub fn get_markdown_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.markdown_path_template(), doc_id, settings)
}

pub fn get_info_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.info_path_template(), doc_id, settings)
}

pub fn get_pdf_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.pdf_path_template(), doc_id, settings)
}

pub fn get_asset_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.markdown_asset_path_template(), doc_id, settings)
}

pub fn get_crossref_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.crossref_path_template(), doc_id, settings)
}

pub fn get_markdown_path_buf(settings: &Settings, doc_id: i64) -> PathBuf {
    PathBuf::from(get_markdown_path(settings, doc_id))
}

pub fn get_info_path_buf(settings: &Settings, doc_id: i64) -> PathBuf {
    PathBuf::from(get_info_path(settings, doc_id))
}

pub fn get_pdf_path_buf(settings: &Settings, doc_id: i64) -> PathBuf {
    PathBuf::from(get_pdf_path(settings, doc_id))
}

pub fn get_asset_path_buf(settings: &Settings, doc_id: i64) -> PathBuf {
    PathBuf::from(get_asset_path(settings, doc_id))
}

pub fn get_crossref_path_buf(settings: &Settings, doc_id: i64) -> PathBuf {
    PathBuf::from(get_crossref_path(settings, doc_id))
}

/// 问答会话文件路径（`{id}` 为会话 ID，字符串）
pub fn get_chat_session_path(settings: &Settings, session_id: &str) -> String {
    crate::config_manager::ConfigManager::substitute_path_variables_str(
        &settings.chat_session_path_template(),
        session_id,
    )
}

pub fn get_chat_session_path_buf(settings: &Settings, session_id: &str) -> PathBuf {
    PathBuf::from(get_chat_session_path(settings, session_id))
}

/// 会话文件所在目录（从模板推导：把 `{id}` 换成占位名后取父目录）
pub fn get_chat_session_dir(settings: &Settings) -> Option<PathBuf> {
    let template = crate::config_manager::ConfigManager::substitute_path_variables_str(
        &settings.chat_session_path_template(),
        "id",
    );
    PathBuf::from(template).parent().map(|p| p.to_path_buf())
}