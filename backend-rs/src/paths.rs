//! 从 system.json 推导文档相关路径。

use std::path::PathBuf;

use crate::config::Settings;

pub fn resolve_template(template: &str, doc_id: i64) -> String {
    template.replace("{id}", &doc_id.to_string())
}

pub fn get_markdown_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.markdown_path_template(), doc_id)
}

pub fn get_info_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.info_path_template(), doc_id)
}

pub fn get_pdf_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.pdf_path_template(), doc_id)
}

pub fn get_asset_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.markdown_asset_path_template(), doc_id)
}

pub fn get_crossref_path(settings: &Settings, doc_id: i64) -> String {
    resolve_template(&settings.crossref_path_template(), doc_id)
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