//! Datalab Document Intelligence API 客户端
//!
//! Datalab (https://www.datalab.to) 提供文档转换 API，可将 PDF/图片/文档
//! 转换为 Markdown、HTML、JSON 等格式。
//!
//! ## API 工作流
//!
//! 1. **转换提交**：`POST /api/v1/convert`，multipart 上传文件 + 参数
//! 2. **轮询结果**：`GET /api/v1/convert/{request_id}`，直到 status 为 "complete"
//! 3. **结果提取**：响应 body 包含 `markdown` 字段（文本）和 `images` 字段（base64 图片字典）
//!
//! ## 认证
//!
//! 使用 `X-API-Key` header 传递 API Key。

use std::path::Path;

use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

/// Datalab 文档转换客户端
pub struct DatalabClient {
    api_key: String,
    http: reqwest::Client,
}

impl DatalabClient {
    /// 创建 Datalab 客户端
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Datalab API 基础 URL
    fn base_url() -> &'static str {
        "https://www.datalab.to"
    }

    // ════════════════════════════════════════════════════════
    //  公开接口（与 MinerUClient 签名保持一致以便 pipeline 调用）
    // ════════════════════════════════════════════════════════

    /// 提交文档转换任务，返回 request_id
    pub async fn submit_parse_task(&self, file_path: &str) -> anyhow::Result<String> {
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        info!("Datalab 提交转换任务: file={}", file_name);

        let pdf_bytes = tokio::fs::read(file_path).await.map_err(|e| {
            error!("Datalab 读取文件失败 ({}): {}", file_path, e);
            anyhow::anyhow!("Datalab 读取文件失败: {}", e)
        })?;

        // multipart 上传表单
        let part = reqwest::multipart::Part::bytes(pdf_bytes)
            .file_name(file_name.clone())
            .mime_str("application/pdf")
            .map_err(|e| anyhow::anyhow!("创建 multipart 失败: {}", e))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("output_format", "markdown")
            .text("mode", "fast");

        let resp = self
            .http
            .post(format!("{}/api/v1/convert", Self::base_url()))
            .header("X-API-Key", &self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                error!("Datalab 连接失败: {}", e);
                anyhow::anyhow!("Datalab 连接失败: {}", e)
            })?;

        if !resp.status().is_success() {
            let status_code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            error!("Datalab 转换请求失败: HTTP {} - {}", status_code, text.chars().take(300).collect::<String>());
            anyhow::bail!("Datalab 转换请求失败: HTTP {} - {}", status_code, text.chars().take(300).collect::<String>());
        }

        let data: Value = resp.json().await.map_err(|e| {
            error!("Datalab 响应解析失败: {}", e);
            anyhow::anyhow!("Datalab 响应解析失败: {}", e)
        })?;

        let request_id = data["request_id"].as_str()
            .ok_or_else(|| {
                error!("Datalab 响应缺少 request_id: {}", data);
                anyhow::anyhow!("Datalab 响应缺少 request_id")
            })?
            .to_string();

        info!("Datalab 转换任务已提交: request_id={}", request_id);
        Ok(request_id)
    }

    /// 检查任务状态，返回标准化 Value
    /// 返回值格式：{ "_done": bool, "status": str, "markdown": Option<str>,
    ///                "images": Option<object>, "error": Option<str> }
    pub async fn check_task_status(&self, request_id: &str) -> Value {
        let url = format!("{}/api/v1/convert/{}", Self::base_url(), request_id);

        match self
            .http
            .get(&url)
            .header("X-API-Key", &self.api_key)
            .send()
            .await
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!("Datalab 查询状态失败 (request_id={}): HTTP {}", request_id, resp.status());
                    return json!({"_done": false, "status": "unknown"});
                }

                let data: Value = match resp.json().await {
                    Ok(d) => d,
                    Err(e) => {
                        warn!("Datalab 响应解析失败 (request_id={}): {}", request_id, e);
                        return json!({"_done": false, "status": "unknown"});
                    }
                };

                let status = data["status"].as_str().unwrap_or("unknown").to_string();
                let is_done = status == "complete";
                let is_failed = status == "error" || status == "failed";

                if is_failed {
                    let err = data["error"].as_str().unwrap_or("Unknown error");
                    warn!("Datalab 任务 (request_id={}) 失败: {}", request_id, err);
                }

                json!({
                    "_done": is_done || is_failed,
                    "status": if is_done { "completed" } else if is_failed { "failed" } else { &status },
                    "markdown": data.get("markdown"),
                    "images": data.get("images"),
                    "error": data.get("error"),
                })
            }
            Err(e) => {
                warn!("Datalab 连接失败 (request_id={}): {}", request_id, e);
                json!({"_done": false, "status": "unknown"})
            }
        }
    }

    /// 轮询任务直到完成
    pub async fn poll_task(&self, request_id: &str, timeout_secs: u64) -> anyhow::Result<Value> {
        let poll_interval = 3u64;
        let max_polls = timeout_secs / poll_interval;

        info!("Datalab 开始轮询任务: request_id={}, timeout={}s", request_id, timeout_secs);

        for i in 0..max_polls {
            let status = self.check_task_status(request_id).await;
            if status["_done"].as_bool().unwrap_or(false) {
                if status["status"].as_str() == Some("completed") {
                    info!("Datalab 任务完成: request_id={}", request_id);
                    return Ok(status);
                }
                let error = status["error"].as_str().unwrap_or("Unknown error");
                error!("Datalab 任务失败 (request_id={}): {}", request_id, error);
                anyhow::bail!("Datalab 转换失败: {}", error);
            }

            if i % 10 == 0 {
                debug!("Datalab 轮询中 [{}/{}], request_id={}, status={:?}",
                       i + 1, max_polls, request_id, status["status"].as_str());
            }

            tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
        }

        warn!("Datalab 任务超时: request_id={}, timeout={}s", request_id, timeout_secs);
        anyhow::bail!("Datalab 转换超时: {}", request_id);
    }

    /// 获取任务结果，返回 markdown 文件路径
    /// 从响应中提取 markdown 和 base64 图片，保存到 output_dir
    pub async fn get_task_result(
        &self,
        request_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let status = self.check_task_status(request_id).await;

        if status["status"].as_str() != Some("completed") {
            let st = status["status"].as_str().unwrap_or("unknown");
            error!("Datalab 任务未完成 (request_id={}): status={}", request_id, st);
            anyhow::bail!("Datalab 任务未完成 (status: {})", st);
        }

        let markdown = status["markdown"].as_str()
            .ok_or_else(|| {
                error!("Datalab 结果缺少 markdown 字段 (request_id={})", request_id);
                anyhow::anyhow!("Datalab 结果缺少 markdown")
            })?;

        info!("Datalab 结果获取成功, request_id={}, markdown={} 字节, 含{}图片",
              request_id, markdown.len(),
              status["images"].as_object().map(|o| o.len()).unwrap_or(0));

        std::fs::create_dir_all(output_dir)?;

        // 保存图片（base64 解码后写入文件）
        let mut saved_images: Vec<(String, Vec<u8>)> = Vec::new();

        if let Some(images) = status["images"].as_object() {
            for (filename, value) in images {
                let b64_str = value.as_str().unwrap_or("");
                if b64_str.is_empty() {
                    continue;
                }

                // 处理可能的 data URI 前缀
                let b64_data = if let Some(pos) = b64_str.find("base64,") {
                    &b64_str[pos + 7..]
                } else {
                    b64_str
                };

                match base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    b64_data,
                ) {
                    Ok(img_data) => {
                        let img_path = Path::new(output_dir).join(filename);
                        if let Some(parent) = img_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if std::fs::write(&img_path, &img_data).is_ok() {
                            debug!("Datalab 图片保存: {:?} ({} 字节)", img_path, img_data.len());
                            saved_images.push((filename.clone(), img_data));
                        } else {
                            warn!("Datalab 图片写入失败: {:?}", img_path);
                        }
                    }
                    Err(e) => {
                        warn!("Datalab 图片 base64 解码失败 ({}): {}", filename, e);
                    }
                }
            }
        }

        // 所有图片打包为 images.zip
        if !saved_images.is_empty() {
            save_images_zip(output_dir, &saved_images);
        }

        // 保存 markdown 文件
        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, markdown)?;

        info!("Datalab Markdown 已保存到: {:?}", md_path);
        Ok(md_path.to_string_lossy().to_string())
    }
}

/// 将图片列表打包为 images.zip
fn save_images_zip(output_dir: &str, images: &[(String, Vec<u8>)]) {
    let images_zip_path = Path::new(output_dir).join("images.zip");
    match std::fs::File::create(&images_zip_path) {
        Ok(zip_file) => {
            let mut zip_writer = zip::ZipWriter::new(zip_file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (filename, data) in images {
                if zip_writer.start_file(filename, options).is_ok() {
                    use std::io::Write;
                    let _ = zip_writer.write_all(data);
                }
            }
            let _ = zip_writer.finish();
        }
        Err(e) => {
            tracing::warn!("Datalab 无法创建 images.zip: {}", e);
        }
    }
}