//! MinerU PDF 解析服务
//!
//! 支持三种 API 接口，由 config.json 的 type 字段选择：
//!
//! 1. **local**（自部署 V1 API）：连接本地 MinerU 服务（mineru-kit api-server）
//!    端点：http://host:port/v1/...
//!    流程：创建上传 → 上传 → 完成 → 提交任务 → 轮询 → 下载
//!
//! 2. **official**（官方云 V4 精准解析 API）：通过 Token 认证，高精度解析
//!    端点：https://mineru.net/api/v4/...
//!    流程：获取上传 URL → 上传文件 → 系统自动提交 → 轮询任务 → 下载 ZIP
//!
//! 3. **official-lightweight**（官方云 V1 Agent 轻量解析 API）：无需 Token，IP 限频
//!    端点：https://mineru.net/api/v1/agent/parse/...
//!    流程：上传文件 → 轮询任务 → 下载 Markdown

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::config::MinerUConfig;
use crate::services::datalab::DatalabClient;

/// MinerU/Datalab 模式类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MineruType {
    /// 自部署 V1 API 服务（type=local）
    Local,
    /// 官方云 V4 精准解析 API（type=official）
    Official,
    /// 官方云 V1 Agent 轻量解析 API（type=official-lightweight）
    OfficialLightweight,
    /// Datalab Document Intelligence API（type=datalab）
    Datalab,
}

impl MineruType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().replace("-", "_").as_str() {
            "official" | "精准解析" | "精准" => MineruType::Official,
            "official_lightweight" | "official-lightweight" | "轻量解析" | "轻量" | "agent" => {
                MineruType::OfficialLightweight
            }
            "datalab" => MineruType::Datalab,
            _ => MineruType::Local,
        }
    }
}

/// 任务状态标准化结果
struct TaskStatus {
    done: bool,
    error: Option<String>,
    /// V1 API: output_files 列表；V4 API: full_zip_url；轻量 API: markdown_url
    result_data: Value,
}

pub struct MinerUClient {
    base_url: String,
    key: String,
    mineru_type: MineruType,
    tier: String,
    http: reqwest::Client,
    datalab_client: Option<DatalabClient>,
}

impl MinerUClient {
    pub fn new(config: &MinerUConfig) -> Self {
        let mineru_type = MineruType::from_str(&config.mineru_type);
        let base_url = Self::build_base_url(&config.url, mineru_type);

        // Datalab 类型时提前创建 DatalabClient
        let datalab_client = if mineru_type == MineruType::Datalab {
            Some(DatalabClient::new(&config.token))
        } else {
            None
        };

        Self {
            base_url,
            key: config.token.clone(),
            mineru_type,
            tier: config.tier.clone(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            datalab_client,
        }
    }

    pub fn from_config(config: &MinerUConfig) -> Self {
        Self::new(config)
    }

    /// 根据类型构建正确的 API 基础 URL
    fn build_base_url(url: &str, mineru_type: MineruType) -> String {
        let url = url.trim_end_matches('/').to_string();
        match mineru_type {
            // 自部署：base_url 直接为 http://host:port
            MineruType::Local => url,
            // V4/Official：https://mineru.net → https://mineru.net/api
            MineruType::Official | MineruType::OfficialLightweight => {
                if !url.ends_with("/api") {
                    format!("{}/api", url)
                } else {
                    url
                }
            }
            // Datalab：固定使用 https://www.datalab.to，忽略 config 中的 url
            MineruType::Datalab => "https://www.datalab.to".to_string(),
        }
    }

    // ════════════════════════════════════════════════════════
    //  公开接口（统一签名，路由到不同实现）
    // ════════════════════════════════════════════════════════

    /// 提交解析任务，返回 task_id / job_id / request_id
    pub async fn submit_parse_task(&self, file_path: &str) -> anyhow::Result<String> {
        self.submit_parse_task_with_range(file_path, None).await
    }

    /// 带页码范围提交解析任务（`page_range` 形如 `"1-100"`，`None` = 整份文档）
    ///
    /// 各类型的能力差异：
    /// - `local`（V1）：`files[].page_range`
    /// - `official`（V4）：`files[].page_ranges`（**必须放 files[] 里，放顶层会被忽略**）
    /// - `datalab`：multipart 的 `page_range` 字段
    /// - `official-lightweight`：**不支持**页码范围，传了会被忽略（打 warn）
    pub async fn submit_parse_task_with_range(
        &self,
        file_path: &str,
        page_range: Option<&str>,
    ) -> anyhow::Result<String> {
        match self.mineru_type {
            MineruType::Local => self.submit_parse_task_v1(file_path, page_range).await,
            MineruType::Official => self.submit_parse_task_v4(file_path, page_range).await,
            MineruType::OfficialLightweight => {
                if page_range.is_some() {
                    warn!("MinerU 轻量 API 不支持页码范围，忽略分片页码（整份文档提交）");
                }
                self.submit_parse_task_lightweight(file_path).await
            }
            MineruType::Datalab => {
                if let Some(ref client) = self.datalab_client {
                    client.submit_parse_task(file_path, page_range).await
                } else {
                    anyhow::bail!("Datalab 客户端未初始化")
                }
            }
        }
    }

    /// 该类型是否支持页码范围（分片解析的前提）
    pub fn supports_page_range(&self) -> bool {
        !matches!(self.mineru_type, MineruType::OfficialLightweight)
    }

    /// 检查任务状态，返回标准化 Value
    pub async fn check_task_status(&self, task_id: &str) -> Value {
        match self.mineru_type {
            MineruType::Local => self.check_task_status_v1(task_id).await,
            MineruType::Official => self.check_task_status_v4(task_id).await,
            MineruType::OfficialLightweight => self.check_task_status_lightweight(task_id).await,
            MineruType::Datalab => {
                if let Some(ref client) = self.datalab_client {
                    client.check_task_status(task_id).await
                } else {
                    json!({"_done": false, "status": "unknown"})
                }
            }
        }
    }

    /// 轮询任务直到完成
    pub async fn poll_task(&self, task_id: &str, timeout_secs: u64) -> anyhow::Result<Value> {
        // Datalab 类型使用 DatalabClient 的 poll
        if self.mineru_type == MineruType::Datalab {
            if let Some(ref client) = self.datalab_client {
                return client.poll_task(task_id, timeout_secs).await;
            }
            anyhow::bail!("Datalab 客户端未初始化");
        }

        let poll_interval = match self.mineru_type {
            MineruType::Local => 2u64,
            MineruType::Official => 5u64,
            MineruType::OfficialLightweight => 3u64,
            MineruType::Datalab => unreachable!(),
        };
        let max_polls = timeout_secs / poll_interval;
        let task_type = match self.mineru_type {
            MineruType::Local => "V1",
            MineruType::Official => "V4",
            MineruType::OfficialLightweight => "轻量",
            MineruType::Datalab => unreachable!(),
        };

        info!("开始轮询 MinerU {} 任务 task_id={}, timeout={}s, poll_interval={}s",
              task_type, task_id, timeout_secs, poll_interval);

        for i in 0..max_polls {
            let status_result = self.check_task_status(task_id).await;
            if let Some(done) = status_result.get("_done").and_then(|v| v.as_bool()) {
                if done {
                    let status = status_result["status"].as_str().unwrap_or("completed");
                    info!("MinerU {} 任务 task_id={} 完成, status={}", task_type, task_id, status);
                    return Ok(status_result);
                }
            }

            let status = status_result["status"].as_str().unwrap_or("unknown");
            match status {
                "failed" => {
                    let error = status_result["error"]
                        .as_str()
                        .unwrap_or("Unknown error");
                    error!("MinerU {} 任务 task_id={} 失败: {}", task_type, task_id, error);
                    anyhow::bail!("MinerU task failed: {}", error);
                }
                "canceled" => {
                    warn!("MinerU {} 任务 task_id={} 被取消", task_type, task_id);
                    anyhow::bail!("MinerU task was canceled");
                }
                _ => {
                    // 每 10 次轮询输出一次进度日志，避免日志过多
                    if i > 0 && i % 10 == 0 {
                        debug!("MinerU {} 任务 task_id={} 轮询中 [{}/{}], status={}",
                               task_type, task_id, i + 1, max_polls, status);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
                }
            }
        }

        warn!("MinerU {} 任务 task_id={} 超时 ({}s)", task_type, task_id, timeout_secs);
        anyhow::bail!("MinerU task timed out: {}", task_id);
    }

    /// 获取任务结果，返回 markdown 文件路径
    pub async fn get_task_result(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        match self.mineru_type {
            MineruType::Local => self.get_task_result_v1(task_id, output_dir, doc_id).await,
            MineruType::Official => self.get_task_result_v4(task_id, output_dir, doc_id).await,
            MineruType::OfficialLightweight => {
                self.get_task_result_lightweight(task_id, output_dir, doc_id)
                    .await
            }
            MineruType::Datalab => {
                if let Some(ref client) = self.datalab_client {
                    client.get_task_result(task_id, output_dir, doc_id).await
                } else {
                    anyhow::bail!("Datalab 客户端未初始化")
                }
            }
        }
    }

    // ════════════════════════════════════════════════════════
    //  分片解析（超过单次页数上限的 PDF）
    // ════════════════════════════════════════════════════════

    /// 分片解析：按 `chunks` 逐片提交、轮询、下载，最后合并 Markdown 与图片。
    ///
    /// - `total_pages` 用于进度汇报与日志；
    /// - `on_progress(已解析页数, 总页数)` 在每个分片开始/结束以及轮询过程中被调用；
    /// - 返回**合并后**的 Markdown 路径（与单任务结果同形，可直接交给 `finish_parse_result`）。
    ///
    /// 每个分片的结果落到 `output_dir/chunk_NNN/`，合并统一在本函数末尾完成，
    /// 图片同名不同内容时会改名并同步改写正文引用（见 [`merge_chunk_results`]）。
    /// 本地切页产生的临时 PDF（`page_range` 为 `None`）在结果下载后立即删除，不占盘。
    pub async fn parse_pdf_chunks(
        &self,
        output_dir: &str,
        doc_id: Option<i64>,
        timeout_secs: u64,
        total_pages: i64,
        chunks: &[ParseChunk],
        on_progress: &(dyn Fn(i64, i64) + Send + Sync),
    ) -> anyhow::Result<String> {
        if chunks.is_empty() {
            anyhow::bail!("分片列表为空");
        }

        let base = Path::new(output_dir);
        std::fs::create_dir_all(base)?;
        let total = chunks.len();

        let mut md_paths: Vec<String> = Vec::with_capacity(total);
        let mut pages_done: i64 = 0;

        info!(
            "开始分片解析: 共 {} 页，分 {} 片（{}）",
            total_pages,
            total,
            if chunks.iter().all(|c| c.page_range.is_none()) {
                "本地切页"
            } else {
                "按页码范围提交"
            }
        );

        for (idx, chunk) in chunks.iter().enumerate() {
            let chunk_dir = base.join(format!("chunk_{:03}", idx + 1));
            let _ = std::fs::remove_dir_all(&chunk_dir);
            std::fs::create_dir_all(&chunk_dir)?;
            let chunk_dir_str = chunk_dir.to_string_lossy().to_string();

            info!(
                "分片解析 [{}/{}]: {}，task_id 提交中...",
                idx + 1,
                total,
                chunk.label()
            );
            on_progress(pages_done, total_pages);

            let task_id = match self
                .submit_parse_task_with_range(&chunk.file_path, chunk.page_range.as_deref())
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    cleanup_split_file(chunk);
                    anyhow::bail!("分片 {}/{} 提交失败: {}", idx + 1, total, e);
                }
            };
            info!(
                "分片解析 [{}/{}]: 已提交 task_id={}",
                idx + 1,
                total,
                task_id
            );

            let base_pages = pages_done;
            let progress = |extracted: i64| {
                // 分片内的已解析页数 + 之前完成的分片页数 = 全文进度
                let done = (base_pages + extracted).min(total_pages);
                on_progress(done, total_pages);
            };
            let polled = self.poll_chunk_task(&task_id, timeout_secs, &progress).await;
            if let Err(e) = polled {
                cleanup_split_file(chunk);
                return Err(anyhow::anyhow!(
                    "分片 {}/{} 解析失败: {}",
                    idx + 1,
                    total,
                    e
                ));
            }

            let md_path = match self
                .get_task_result(&task_id, &chunk_dir_str, doc_id)
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    cleanup_split_file(chunk);
                    anyhow::bail!(
                        "分片 {}/{} 获取结果失败: {}",
                        idx + 1,
                        total,
                        e
                    );
                }
            };
            info!(
                "分片解析 [{}/{}]: 结果已下载 {}",
                idx + 1,
                total,
                md_path
            );
            md_paths.push(md_path);
            pages_done += chunk.pages;
            on_progress(pages_done.min(total_pages), total_pages);
            // 每一片的临时 PDF 用完就删，大文件不会在临时目录里成倍占盘
            cleanup_split_file(chunk);
        }

        let merged = merge_chunk_results(&md_paths, output_dir, doc_id)?;
        info!("分片解析完成: {} 片已合并 → {}", total, merged);
        Ok(merged)
    }

    /// 轮询单个分片任务，并把 API 回传的已解析页数换算成全局页数进度
    async fn poll_chunk_task(
        &self,
        task_id: &str,
        timeout_secs: u64,
        on_progress: &(dyn Fn(i64) + Send + Sync),
    ) -> anyhow::Result<Value> {
        let poll_interval = match self.mineru_type {
            MineruType::Local => 2u64,
            MineruType::Official => 5u64,
            MineruType::OfficialLightweight => 3u64,
            MineruType::Datalab => 3u64,
        };
        let max_polls = if timeout_secs > 0 {
            timeout_secs / poll_interval
        } else {
            u64::MAX
        };

        for i in 0..max_polls {
            let status_result = self.check_task_status(task_id).await;

            if let Some(progress) = status_result.get("extract_progress") {
                if let Some(extracted) = progress["extracted_pages"].as_i64() {
                    on_progress(extracted.max(0));
                }
            }

            if status_result
                .get("_done")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Ok(status_result);
            }

            match status_result["status"].as_str().unwrap_or("unknown") {
                "failed" => {
                    let error = status_result["error"].as_str().unwrap_or("Unknown error");
                    anyhow::bail!("MinerU task failed: {}", error);
                }
                "canceled" => anyhow::bail!("MinerU task was canceled"),
                status => {
                    if i > 0 && i % 10 == 0 {
                        debug!(
                            "分片任务 task_id={} [{}/{}], status={}",
                            task_id,
                            i + 1,
                            max_polls,
                            status
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
                }
            }
        }

        anyhow::bail!("MinerU 分片任务超时: {}", task_id);
    }

    // ════════════════════════════════════════════════════════
    //  实现：V1 自部署 API（type=local）
    // ════════════════════════════════════════════════════════

    async fn submit_parse_task_v1(
        &self,
        file_path: &str,
        page_range: Option<&str>,
    ) -> anyhow::Result<String> {
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        // 流式读盘：只算大小与 sha256，不把整份 PDF 留在内存
        let (file_size, sha256_hex) = file_fingerprint(file_path).await?;
        let (upload_id, upload_method, upload_url, upload_headers) = self
            .v1_create_upload(&file_name, file_size, &sha256_hex)
            .await?;

        if upload_method != "SKIP" {
            self.v1_upload_file(&upload_method, &upload_url, &upload_headers, file_path, file_size)
                .await?;
        }

        let file_id = self.v1_complete_upload(&upload_id).await?;
        let job_id = self.v1_submit_job(&file_id, page_range).await?;
        Ok(job_id)
    }

    /// V1: 创建上传
    async fn v1_create_upload(
        &self,
        filename: &str,
        file_size: u64,
        sha256_hex: &str,
    ) -> anyhow::Result<(String, String, String, HashMap<String, String>)> {
        let sha256_hex = sha256_hex.to_string();

        let body = json!({
            "filename": filename,
            "bytes": file_size,
            "sha256sum": sha256_hex,
            "mime_type": "application/pdf",
            "purpose": "parse",
        });

        let resp = self
            .http
            .post(format!("{}/v1/uploads", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("MinerU create upload failed: HTTP {} - {}", status, text.chars().take(200).collect::<String>());
        }

        let data: Value = resp.json().await?;

        if data.get("file").and_then(|v| v.as_object()).is_some() {
            let file_id = data["file"]["id"].as_str()
                .ok_or_else(|| anyhow::anyhow!("No file.id in dedup response"))?
                .to_string();
            return Ok((file_id, "SKIP".to_string(), String::new(), HashMap::new()));
        }

        let upload_id = data["id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No id in response"))?
            .to_string();
        let upload_method = data["upload_method"].as_str().unwrap_or("PUT").to_string();
        let upload_url = data["upload_url"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No upload_url in response"))?
            .to_string();

        let mut upload_headers = HashMap::new();
        if let Some(headers) = data["upload_headers"].as_object() {
            for (k, v) in headers {
                if let Some(val) = v.as_str() {
                    upload_headers.insert(k.clone(), val.to_string());
                }
            }
        }

        Ok((upload_id, upload_method, upload_url, upload_headers))
    }

    /// V1: 上传文件（流式：直接读磁盘文件作为请求体，不先读入内存）
    async fn v1_upload_file(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        data_path: &str,
        data_len: u64,
    ) -> anyhow::Result<()> {
        if method == "SKIP" {
            return Ok(());
        }

        let full_url = resolve_url(&self.base_url, url);
        let mut req = match method.to_uppercase().as_str() {
            "PUT" => self.http.put(&full_url),
            "POST" => self.http.post(&full_url),
            _ => self.http.put(&full_url),
        };

        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }

        if !self.key.is_empty() && is_same_origin(&full_url, &self.base_url) {
            req = req.bearer_auth(&self.key);
        }

        let req = stream_file_body(req, data_path, data_len).await?;
        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("MinerU file upload failed: HTTP {}", resp.status());
        }
        Ok(())
    }

    /// V1: 完成上传 → file_id
    async fn v1_complete_upload(&self, upload_id: &str) -> anyhow::Result<String> {
        let resp = self
            .http
            .post(format!("{}/v1/uploads/{}/complete", self.base_url, upload_id))
            .bearer_auth(&self.key)
            .json(&json!({}))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("MinerU complete upload failed: HTTP {}", resp.status());
        }
        let data: Value = resp.json().await?;
        let file_id = data["file"]["id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No file.id in complete response"))?
            .to_string();
        Ok(file_id)
    }

    /// V1: 提交解析任务 → job_id
    async fn v1_submit_job(&self, file_id: &str, page_range: Option<&str>) -> anyhow::Result<String> {
        let mut file_entry = json!({
            "source": { "type": "file_id", "file_id": file_id }
        });
        // `files[].page_range`：分片解析时只取这一段页码（仅 PDF 支持）
        if let Some(range) = page_range {
            file_entry["page_range"] = json!(range);
        }
        let body = json!({
            "files": [file_entry],
            "tier": self.tier,
            "output_formats": ["markdown", "zip"],
        });

        let resp = self
            .http
            .post(format!("{}/v1/parse/jobs", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("MinerU submit job failed: HTTP {} - {}", status, text.chars().take(200).collect::<String>());
        }
        let data: Value = resp.json().await?;
        Ok(data["job_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No job_id in response"))?
            .to_string())
    }

    /// V1: 检查任务状态
    async fn check_task_status_v1(&self, job_id: &str) -> Value {
        let resp = self
            .http
            .get(format!("{}/v1/parse/jobs/{}", self.base_url, job_id))
            .bearer_auth(&self.key)
            .send()
            .await;

        match resp {
            Ok(resp) if resp.status().is_success() => {
                let data: Value = resp.json().await.unwrap_or(json!({}));
                let status = data["status"].as_str().unwrap_or("unknown");
                let normalized = match status {
                    "completed" => "completed",
                    "failed" => "failed",
                    "queued" => "pending",
                    "running" => "processing",
                    "processing" => "processing",
                    "partial" => "partial",
                    "canceled" => "canceled",
                    _ => status,
                };

                let files = data["files"].as_array().cloned().unwrap_or_default();
                let mut output_files: Vec<Value> = Vec::new();
                for file_entry in &files {
                    if let Some(of) = file_entry.get("output_files").and_then(|v| v.as_object()) {
                        for (fmt_name, fmt_val) in of {
                            if let Some(fid) = fmt_val.get("file_id").and_then(|v| v.as_str()) {
                                output_files.push(json!({
                                    "file_id": fid,
                                    "format": fmt_name,
                                    "bytes": fmt_val.get("bytes"),
                                }));
                            }
                        }
                    }
                }

                json!({
                    "_done": normalized == "completed" || normalized == "partial",
                    "status": normalized,
                    "output_files": output_files,
                    "error": data.get("last_error").or_else(|| data.get("error")).cloned().unwrap_or(json!(null)),
                })
            }
            _ => json!({"_done": false, "status": "unknown"}),
        }
    }

    /// V1: 获取任务结果
    async fn get_task_result_v1(
        &self,
        job_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let status_result = self.check_task_status_v1(job_id).await;
        let status = status_result["status"].as_str().unwrap_or("");
        if status != "completed" && status != "partial" {
            anyhow::bail!("Task not completed (status: {})", status);
        }

        let output_files = status_result["output_files"].as_array().cloned().unwrap_or_default();
        if output_files.is_empty() {
            anyhow::bail!("No output files in job result");
        }

        std::fs::create_dir_all(output_dir)?;

        let mut zip_bytes: Option<Vec<u8>> = None;
        let mut standalone_markdown: Option<String> = None;

        for file_entry in &output_files {
            let format = file_entry["format"].as_str().unwrap_or("");
            let file_id = file_entry["file_id"].as_str().unwrap_or("");
            if file_id.is_empty() {
                continue;
            }

            let content = match self.v1_download_file(file_id).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("下载文件 {} 失败: {}", file_id, e);
                    continue;
                }
            };

            match format {
                "markdown" => standalone_markdown = Some(String::from_utf8_lossy(&content).to_string()),
                "zip" => zip_bytes = Some(content),
                _ => {}
            }
        }

        let (final_markdown, extracted_images) = if let Some(zip_data) = &zip_bytes {
            extract_from_zip(zip_data, output_dir)
        } else {
            (None, Vec::new())
        };

        let md = if let Some(ref md_from_zip) = final_markdown {
            md_from_zip.clone()
        } else if let Some(ref md_standalone) = standalone_markdown {
            md_standalone.clone()
        } else {
            anyhow::bail!("No markdown content found in MinerU result");
        };

        if !extracted_images.is_empty() {
            save_images_zip(output_dir, &extracted_images);
        }

        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, &md)?;
        Ok(md_path.to_string_lossy().to_string())
    }

    /// V1: 下载文件（支持 302 跳转）
    async fn v1_download_file(&self, file_id: &str) -> anyhow::Result<Vec<u8>> {
        let url = format!("{}/v1/files/{}/content", self.base_url, file_id);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.key)
            .send()
            .await
            .map_err(|e| {
                error!("V1 下载文件连接失败 (file_id={}): {}", file_id, e);
                anyhow::anyhow!("MinerU 下载文件连接失败: {}", e)
            })?;

        if resp.status().is_success() {
            let bytes = resp.bytes().await.map_err(|e| {
                error!("V1 读取文件内容失败 (file_id={}): {}", file_id, e);
                anyhow::anyhow!("读取文件内容失败: {}", e)
            })?.to_vec();
            info!("V1 文件下载成功, file_id={}, {} 字节", file_id, bytes.len());
            return Ok(bytes);
        }

        if matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            if let Some(location) = resp.headers().get("location").and_then(|v| v.to_str().ok()) {
                info!("V1 文件下载重定向: {} -> {}", url, location);
                let redirect_resp = self.http.get(location).send().await.map_err(|e| {
                    error!("V1 下载文件重定向连接失败: {}", e);
                    anyhow::anyhow!("下载重定向连接失败: {}", e)
                })?;
                if redirect_resp.status().is_success() {
                    let bytes = redirect_resp.bytes().await.map_err(|e| {
                        error!("V1 读取重定向文件内容失败: {}", e);
                        anyhow::anyhow!("读取重定向文件内容失败: {}", e)
                    })?.to_vec();
                    info!("V1 文件下载成功 (重定向), file_id={}, {} 字节", file_id, bytes.len());
                    return Ok(bytes);
                }
                warn!("V1 文件下载重定向失败: HTTP {}", redirect_resp.status());
                anyhow::bail!("MinerU file download redirect failed: HTTP {}", redirect_resp.status());
            }
        }

        error!("V1 文件下载失败 (file_id={}): HTTP {}", file_id, resp.status());
        anyhow::bail!("MinerU file download failed: HTTP {}", resp.status());
    }

    // ════════════════════════════════════════════════════════
    //  实现：V4 官方精准解析 API（type=official）
    // ════════════════════════════════════════════════════════

    /// V4: 通过批量文件上传提交任务
    /// 流程：获取预签名上传 URL → 上传文件 → 提交解析任务 → 轮询 → 获取结果
    /// 返回 submit_batch_id（用于后续轮询任务状态）
    async fn submit_parse_task_v4(
        &self,
        file_path: &str,
        page_range: Option<&str>,
    ) -> anyhow::Result<String> {
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        let data_id = format!("okb_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs());

        // 流式读盘：只算大小，上传时直接以磁盘文件为请求体
        let file_size = tokio::fs::metadata(file_path).await?.len();

        // Step 1: 获取预签名上传 URL
        // 注意：model_version 参数已被官方 V4 API 移除，不再发送
        // `files[].page_ranges`：分片解析时只解析这一段页码，无需本地切 PDF。
        // 实测（2026-11）页码必须放在 **files[] 条目里**，放顶层会被静默忽略，
        // 结果仍是整份文档（extract_progress.total_pages 依旧是全文页数）。
        // 代价是每个分片都要重新上传一次整份文件（API 限制）。
        let mut file_entry = json!({
            "name": file_name,
            "data_id": data_id,
        });
        if let Some(range) = page_range {
            file_entry["page_ranges"] = json!(range);
        }
        let body = json!({
            "files": [file_entry],
        });

        debug!("V4 请求上传 URL: {}", body.to_string());

        let resp = self
            .http
            .post(format!("{}/v4/file-urls/batch", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error!("V4 连接 MinerU 官方 API 失败 (url={}): {}", self.base_url, e);
                anyhow::bail!("V4 连接失败: {}", e);
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            error!("V4 获取上传 URL 失败: HTTP {} - {}", status, text.chars().take(300).collect::<String>());
            anyhow::bail!("V4 获取上传 URL 失败: HTTP {} - {}", status, text.chars().take(300).collect::<String>());
        }

        let data: Value = resp.json().await.unwrap_or_default();
        if data["code"].as_i64().unwrap_or(-1) != 0 {
            error!("V4 获取上传 URL 失败 (code={}): {}", data["code"], data["msg"].as_str().unwrap_or("unknown error"));
            anyhow::bail!("V4 获取上传 URL 失败: {}", data["msg"].as_str().unwrap_or("unknown error"));
        }

        let batch_id = data["data"]["batch_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("V4 响应缺少 batch_id"))?
            .to_string();

        let file_urls = data["data"]["file_urls"].as_array()
            .ok_or_else(|| anyhow::anyhow!("V4 响应缺少 file_urls"))?;

        if file_urls.is_empty() {
            anyhow::bail!("V4 响应 file_urls 为空");
        }

        let upload_url = file_urls[0].as_str()
            .ok_or_else(|| anyhow::anyhow!("V4 file_urls[0] 不是字符串"))?;

        // Step 2: 上传文件到预签名 URL
        // 阿里云 OSS 预签名 URL 的签名不含 Content-Type，需移除默认 Content-Type header
        let put_req = self
            .http
            .put(upload_url)
            .header("Content-Type", "");
        let put_req = match stream_file_body(put_req, file_path, file_size).await {
            Ok(r) => r,
            Err(e) => {
                error!("V4 打开待上传文件失败 (file={}): {}", file_name, e);
                anyhow::bail!("V4 打开文件失败: {}", e);
            }
        };
        let upload_resp = put_req.send().await;

        match &upload_resp {
            Ok(r) if r.status().is_success() => {
                info!("V4 文件上传成功, batch_id={}, file={}", batch_id, file_name);
            }
            Ok(r) => {
                error!("V4 文件上传失败: HTTP {}, batch_id={}, file={}", r.status(), batch_id, file_name);
                anyhow::bail!("V4 文件上传失败: HTTP {}", r.status());
            }
            Err(e) => {
                error!("V4 文件上传失败 (连接错误), batch_id={}, file={}: {}", batch_id, file_name, e);
                anyhow::bail!("V4 文件上传失败 (连接错误): {}", e);
            }
        }

        let _ = upload_resp; // 消掉 unused 警告

        // 说明：MinerU 官方 V4 API 上传文件后，系统会自动提交解析任务，
        // 无需额外调用提交接口。使用 upload batch_id 通过新的 extract-results 端点查询状态。
        // 参考：https://mineru.net/apiManage/docs

        info!("V4 解析任务已自动提交, batch_id={}, file={}", batch_id, file_name);

        // 返回 upload batch_id 用于轮询任务状态（使用 /v4/extract-results/batch/{batch_id}）
        Ok(batch_id)
    }

    /// V4: 检查任务状态
    /// 使用 batch_id 查询 batch 中所有 task 的状态
    /// 注意：MinerU 官方已将端点从 /v4/extract/task/batch?batch_id= 改为 /v4/extract-results/batch/{batch_id}（路径参数）
    async fn check_task_status_v4(&self, batch_id: &str) -> Value {
        // V4 batch 任务查询：GET /api/v4/extract-results/batch/{batch_id}
        // 返回 batch 中所有 task 的状态
        let batch_query_url = format!("{}/v4/extract-results/batch/{}", self.base_url, batch_id);
        let resp = self.http.get(&batch_query_url)
            .bearer_auth(&self.key)
            .send()
            .await;

        match resp {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!("V4 查询任务状态失败 (batch_id={}): HTTP {}", batch_id, resp.status());
                    return json!({"_done": false, "status": "pending", "batch_id": batch_id});
                }

                let data: Value = match resp.json().await {
                    Ok(d) => d,
                    Err(e) => {
                        warn!("V4 查询任务状态响应解析失败 (batch_id={}): {}", batch_id, e);
                        return json!({"_done": false, "status": "pending", "batch_id": batch_id});
                    }
                };

                let code = data["code"].as_i64().unwrap_or(-1);
                if code != 0 {
                    debug!("V4 查询任务状态返回非零 code (batch_id={}): code={}, msg={}",
                           batch_id, code, data["msg"].as_str().unwrap_or(""));
                    return json!({"_done": false, "status": "pending", "batch_id": batch_id});
                }

                // 新 API 响应使用 extract_result 数组（MinerU 官方已更新）
                if let Some(tasks) = data["data"]["extract_result"].as_array() {
                    if tasks.is_empty() {
                        debug!("V4 任务状态查询 (batch_id={}): extract_result 为空，继续等待", batch_id);
                        return json!({"_done": false, "status": "pending", "batch_id": batch_id});
                    }

                    // 检查所有 task 的状态
                    let all_done = tasks.iter().all(|t| {
                        t["state"].as_str() == Some("done") || t["state"].as_str() == Some("failed")
                    });
                    let any_failed = tasks.iter().any(|t| t["state"].as_str() == Some("failed"));
                    let any_running = tasks.iter().any(|t| {
                        matches!(t["state"].as_str(), Some("running") | Some("extract") | Some("converting"))
                    });

                    if all_done && !any_failed {
                        // 全部完成且无失败
                        let first_done = tasks.iter().find(|t| t["state"].as_str() == Some("done"));
                        let zip_url = first_done.and_then(|t| t["full_zip_url"].as_str());
                        info!("V4 任务 (batch_id={}) 解析完成, zip_url 可用: {}",
                              batch_id, zip_url.map(|u| "yes").unwrap_or("no"));
                        return json!({
                            "_done": true,
                            "status": "completed",
                            "batch_id": batch_id,
                            "full_zip_url": zip_url,
                        });
                    } else if all_done && any_failed {
                        // 全部结束但有失败
                        let errors: Vec<String> = tasks.iter()
                            .filter_map(|t| t["err_msg"].as_str().map(|s| s.to_string()))
                            .collect();
                        warn!("V4 任务 (batch_id={}) 解析失败: {}", batch_id, errors.join("; "));
                        return json!({
                            "_done": true,
                            "status": "failed",
                            "error": errors.join("; "),
                            "batch_id": batch_id,
                        });
                    } else if any_running {
                        debug!("V4 任务 (batch_id={}) 正在解析中, task_count={}", batch_id, tasks.len());
                        // 从第一个运行中的任务提取进度信息
                        let first_running = tasks.iter().find(|t| {
                            matches!(t["state"].as_str(), Some("running") | Some("extract") | Some("converting"))
                        });
                        let extract_progress = first_running
                            .and_then(|t| t.get("extract_progress"))
                            .and_then(|p| {
                                let extracted = p["extracted_pages"].as_i64();
                                let total = p["total_pages"].as_i64();
                                extracted.zip(total).map(|(e, t)| json!({
                                    "extracted_pages": e,
                                    "total_pages": t,
                                }))
                            });
                        let mut result = json!({
                            "_done": false,
                            "status": "processing",
                            "batch_id": batch_id,
                            "task_count": tasks.len(),
                        });
                        if let Some(progress) = extract_progress {
                            result["extract_progress"] = progress;
                        }
                        return result;
                    } else {
                        // 没有运行中的任务，也没有完成的，可能还在排队
                        debug!("V4 任务 (batch_id={}) 排队中...", batch_id);
                        return json!({
                            "_done": false,
                            "status": "pending",
                            "batch_id": batch_id,
                        });
                    }
                } else {
                    // response 有 data 但无 extract_result 字段，可能 batch 还在处理中
                    debug!("V4 任务状态查询 (batch_id={}): data 中无 extract_result 字段", batch_id);
                    return json!({"_done": false, "status": "pending", "batch_id": batch_id});
                }
            }
            Err(e) => {
                warn!("V4 连接 MinerU API 查询任务状态失败 (batch_id={}): {}", batch_id, e);
                json!({
                    "_done": false,
                    "status": "pending",
                    "batch_id": batch_id,
                })
            }
        }
    }

    /// V4: 获取任务结果
    /// 从 status result 中提取 full_zip_url 并下载
    async fn get_task_result_v4(
        &self,
        batch_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        std::fs::create_dir_all(output_dir)?;

        // 先查询 batch 状态获取 zip_url
        let status = self.check_task_status_v4(batch_id).await;

        // 如果有 full_zip_url 则直接下载
        if let Some(zip_url) = status["full_zip_url"].as_str() {
            info!("V4 正在下载解析结果 ZIP: batch_id={}", batch_id);
            return self.v4_download_and_extract_zip(zip_url, output_dir, doc_id).await;
        }

        if status["status"].as_str() == Some("failed") {
            let err = status["error"].as_str().unwrap_or("未知错误");
            error!("V4 解析失败 (batch_id={}): {}", batch_id, err);
            anyhow::bail!("V4 解析失败: {}", err);
        }

        // 仍然没有结果
        warn!("V4 任务尚未完成 (batch_id={}), status={:?}", batch_id, status["status"].as_str());
        anyhow::bail!("V4 任务尚未完成 (batch_id={})", batch_id);
    }

    /// V4: 设置完成状态（供外部调用，当轮询找到完成的 task 时设置）
    /// 这个函数被 v4_inject_task_result 使用
    async fn v4_download_and_extract_zip(
        &self,
        zip_url: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        info!("正在从 {} 下载解析结果 ZIP", zip_url.chars().take(80).collect::<String>());

        // 从 CDN 下载 ZIP
        let resp = self.http.get(zip_url).send().await.map_err(|e| {
            error!("下载 ZIP 连接失败: {}", e);
            anyhow::anyhow!("下载 ZIP 连接失败: {}", e)
        })?;

        if !resp.status().is_success() {
            error!("下载 ZIP 失败: HTTP {}, url={}", resp.status(), zip_url.chars().take(80).collect::<String>());
            anyhow::bail!("下载 ZIP 失败: HTTP {}", resp.status());
        }

        let zip_bytes = resp.bytes().await.map_err(|e| {
            error!("读取 ZIP 响应体失败: {}", e);
            anyhow::anyhow!("读取 ZIP 响应体失败: {}", e)
        })?.to_vec();

        info!("ZIP 下载成功, 大小: {} 字节", zip_bytes.len());

        // 从 ZIP 提取 markdown 和图片
        let (final_markdown, extracted_images) = extract_from_zip(&zip_bytes, output_dir);

        let md = match final_markdown {
            Some(m) => {
                info!("成功从 ZIP 提取 Markdown, {} 字节", m.len());
                m
            }
            None => {
                error!("ZIP 中未找到 markdown 文件");
                anyhow::bail!("ZIP 中未找到 markdown 文件");
            }
        };

        if !extracted_images.is_empty() {
            info!("从 ZIP 提取了 {} 张图片", extracted_images.len());
            save_images_zip(output_dir, &extracted_images);
        }

        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, &md)?;

        info!("Markdown 已保存到: {:?}", md_path);
        Ok(md_path.to_string_lossy().to_string())
    }

    /// V4: 直接通过 task_id 查询任务状态（用于轮询找到 task_id 后的查询）
    pub async fn v4_query_task(&self, task_id: &str) -> Value {
        let url = format!("{}/v4/extract/task/{}", self.base_url, task_id);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.key)
            .send()
            .await;

        match resp {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!("V4 查询单任务状态失败 (task_id={}): HTTP {}", task_id, resp.status());
                    return json!({"_done": false, "status": "unknown"});
                }
                let data: Value = match resp.json().await {
                    Ok(d) => d,
                    Err(e) => {
                        warn!("V4 查询单任务状态响应解析失败 (task_id={}): {}", task_id, e);
                        return json!({"_done": false, "status": "unknown"});
                    }
                };
                if data["code"].as_i64().unwrap_or(-1) != 0 {
                    warn!("V4 查询单任务状态返回非零 code (task_id={}): code={}, msg={}",
                          task_id, data["code"].as_i64().unwrap_or(-1), data["msg"].as_str().unwrap_or(""));
                    return json!({"_done": false, "status": "unknown"});
                }
                let state = data["data"]["state"].as_str().unwrap_or("unknown");
                let normalized = match state {
                    "done" => "completed",
                    "failed" => "failed",
                    "pending" | "queued" => "pending",
                    "running" | "extract" | "converting" => "processing",
                    _ => state,
                };
                let is_done = state == "done" || state == "failed";
                if state == "failed" {
                    let err = data["data"]["err_msg"].as_str().unwrap_or("");
                    warn!("V4 单个任务 (task_id={}) 解析失败: {}", task_id, err);
                }
                json!({
                    "_done": is_done,
                    "status": normalized,
                    "full_zip_url": data["data"].get("full_zip_url"),
                    "error": data["data"].get("err_msg").and_then(|v| v.as_str()).filter(|s| !s.is_empty()),
                    "task_id": task_id,
                })
            }
            Err(e) => {
                warn!("V4 连接 MinerU API 查询单任务状态失败 (task_id={}): {}", task_id, e);
                json!({"_done": false, "status": "unknown"})
            }
        }
    }

    /// V4: 通过 task_id 下载结果
    pub async fn v4_get_result_by_task(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let status = self.v4_query_task(task_id).await;
        let zip_url = status["full_zip_url"].as_str()
            .ok_or_else(|| anyhow::anyhow!("任务未完成或无 ZIP 下载链接"))?;
        self.v4_download_and_extract_zip(zip_url, output_dir, doc_id).await
    }

    // ════════════════════════════════════════════════════════
    //  实现：V1 Agent 轻量解析 API（type=official-lightweight）
    // ════════════════════════════════════════════════════════

    /// 轻量 API: 上传文件并提交解析
    async fn submit_parse_task_lightweight(&self, file_path: &str) -> anyhow::Result<String> {
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        info!("轻量 API 开始提交解析任务: file={}", file_name);

        // 使用 multipart 流式上传文件（不把整份 PDF 读入内存）
        let part = stream_file_part(file_path, &file_name).await.map_err(|e| {
            error!("轻量 API 打开 PDF 文件失败 ({}): {}", file_path, e);
            anyhow::anyhow!("读取 PDF 文件失败: {}", e)
        })?;

        let form = reqwest::multipart::Form::new()
            .part("file", part);

        let resp = self
            .http
            .post(format!("{}/v1/agent/parse/file", self.base_url))
            .multipart(form)
            .send()
            .await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error!("轻量 API 连接 MinerU 服务失败: {}", e);
                anyhow::bail!("轻量 API 连接失败: {}", e);
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            error!("轻量 API 上传失败: HTTP {} - {}", status, text.chars().take(300).collect::<String>());
            anyhow::bail!("轻量 API 上传失败: HTTP {} - {}", status, text.chars().take(300).collect::<String>());
        }

        let data: Value = resp.json().await.unwrap_or_default();
        if data["code"].as_i64().unwrap_or(-1) != 0 {
            error!("轻量 API 提交失败: code={}, msg={}", data["code"].as_i64().unwrap_or(-1), data["msg"].as_str().unwrap_or(""));
            anyhow::bail!("轻量 API 提交失败: {}", data["msg"].as_str().unwrap_or("unknown error"));
        }

        let task_id = data["data"]["task_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("轻量 API 响应缺少 task_id"))?
            .to_string();

        info!("轻量 API 任务已提交: task_id={}", task_id);
        Ok(task_id)
    }

    /// 轻量 API: 检查任务状态
    async fn check_task_status_lightweight(&self, task_id: &str) -> Value {
        let url = format!("{}/v1/agent/parse/{}", self.base_url, task_id);
        let resp = self
            .http
            .get(&url)
            .send()
            .await;

        match resp {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!("轻量 API 查询任务状态失败 (task_id={}): HTTP {}", task_id, resp.status());
                    return json!({"_done": false, "status": "unknown"});
                }
                let data: Value = match resp.json().await {
                    Ok(d) => d,
                    Err(e) => {
                        warn!("轻量 API 查询任务状态响应解析失败 (task_id={}): {}", task_id, e);
                        return json!({"_done": false, "status": "unknown"});
                    }
                };
                if data["code"].as_i64().unwrap_or(-1) != 0 {
                    warn!("轻量 API 查询任务状态返回非零 code (task_id={}): code={}, msg={}",
                          task_id, data["code"].as_i64().unwrap_or(-1), data["msg"].as_str().unwrap_or(""));
                    return json!({"_done": false, "status": "unknown"});
                }
                let state = data["data"]["state"].as_str().unwrap_or("unknown");
                let normalized = match state {
                    "done" => "completed",
                    "failed" => "failed",
                    "pending" | "queued" => "pending",
                    "running" | "extracting" => "processing",
                    _ => state,
                };
                if state == "failed" {
                    let err = data["data"]["err_msg"].as_str().unwrap_or("");
                    warn!("轻量 API 任务 (task_id={}) 解析失败: {}", task_id, err);
                }
                json!({
                    "_done": state == "done" || state == "failed",
                    "status": normalized,
                    "markdown_url": data["data"].get("markdown_url").or_else(|| data["data"].get("url")),
                    "error": data["data"].get("err_msg").and_then(|v| v.as_str()).filter(|s| !s.is_empty()),
                    "task_id": task_id,
                })
            }
            Err(e) => {
                warn!("轻量 API 连接 MinerU 服务失败 (task_id={}, url={}): {}", task_id, url, e);
                json!({"_done": false, "status": "unknown"})
            }
        }
    }

    /// 轻量 API: 获取任务结果
    async fn get_task_result_lightweight(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let status = self.check_task_status_lightweight(task_id).await;
        if status["status"].as_str() != Some("completed") {
            let st = status["status"].as_str().unwrap_or("unknown");
            error!("轻量 API 任务未完成 (task_id={}): status={}", task_id, st);
            anyhow::bail!("轻量 API 任务未完成 (status: {})", st);
        }

        // 获取 markdown URL
        let md_url = status["markdown_url"].as_str()
            .or_else(|| {
                // 也尝试从 data 中获取 url
                status.get("data")
                    .and_then(|d| d.get("url"))
                    .and_then(|v| v.as_str())
            })
            .ok_or_else(|| {
                error!("轻量 API 结果缺少 markdown_url (task_id={})", task_id);
                anyhow::anyhow!("轻量 API 结果缺少 markdown_url")
            })?;

        info!("轻量 API 正在下载解析结果: task_id={}", task_id);

        // 下载 markdown
        let resp = self.http.get(md_url).send().await.map_err(|e| {
            error!("轻量 API 下载 markdown 连接失败: {}", e);
            anyhow::anyhow!("下载 markdown 连接失败: {}", e)
        })?;

        if !resp.status().is_success() {
            error!("轻量 API 下载 markdown 失败: HTTP {}", resp.status());
            anyhow::bail!("下载 markdown 失败: HTTP {}", resp.status());
        }

        let md_content = resp.text().await.map_err(|e| {
            error!("轻量 API 读取 markdown 响应体失败: {}", e);
            anyhow::anyhow!("读取 markdown 响应体失败: {}", e)
        })?;

        info!("轻量 API Markdown 下载成功, {} 字节", md_content.len());

        std::fs::create_dir_all(output_dir)?;

        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, &md_content)?;

        info!("轻量 API Markdown 已保存到: {:?}", md_path);
        Ok(md_path.to_string_lossy().to_string())
    }
}

// ════════════════════════════════════════════════════════
//  便捷函数
// ════════════════════════════════════════════════════════

/// 解析 PDF（提交 → 轮询 → 获取结果）
pub async fn parse_pdf(
    client: &MinerUClient,
    file_path: &str,
    output_dir: &str,
    doc_id: Option<i64>,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    info!("parse_pdf: 开始解析 PDF, file_path={}", file_path);
    let task_id = client.submit_parse_task(file_path).await.map_err(|e| {
        error!("parse_pdf: 提交解析任务失败: {}", e);
        e
    })?;
    info!("parse_pdf: 任务已提交, task_id={}", task_id);

    client.poll_task(&task_id, timeout_secs).await.map_err(|e| {
        error!("parse_pdf: 轮询任务失败 (task_id={}): {}", task_id, e);
        e
    })?;
    info!("parse_pdf: 任务完成, 正在获取结果");

    let result = client.get_task_result(&task_id, output_dir, doc_id).await.map_err(|e| {
        error!("parse_pdf: 获取结果失败 (task_id={}): {}", task_id, e);
        e
    })?;
    info!("parse_pdf: 解析完成, markdown_path={:?}", result);
    Ok(result)
}

/// 使用第一个启用的 MinerU 配置解析 PDF
pub async fn parse_pdf_with_fallback(
    configs: &[MinerUConfig],
    file_path: &str,
    output_dir: &str,
    doc_id: Option<i64>,
) -> anyhow::Result<(usize, String)> {
    let config = configs.first()
        .ok_or_else(|| anyhow::anyhow!("没有可用的 MinerU 配置"))?;
    let client = MinerUClient::new(config);
    let path = parse_pdf(&client, file_path, output_dir, doc_id, config.task_timeout).await?;
    Ok((0, path))
}

// ════════════════════════════════════════════════════════
//  通用工具函数
// ════════════════════════════════════════════════════════

/// 流式计算 `(文件字节数, sha256)`，不在内存里留整份 PDF。
async fn file_fingerprint(path: &str) -> anyhow::Result<(u64, String)> {
    let size = tokio::fs::metadata(path).await?.len();
    let hash = crate::utils::calculate_file_hash_async(path.to_string()).await?;
    Ok((size, hash))
}

/// 把一个磁盘文件流式挂到请求体上。
///
/// 流式 body 本身不带长度，必须显式给 `Content-Length`：否则会退化为
/// `Transfer-Encoding: chunked`，预签名上传 URL（OSS 等）会校验失败。
async fn stream_file_body(
    req: reqwest::RequestBuilder,
    path: &str,
    len: u64,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let file = tokio::fs::File::open(path).await?;
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
    Ok(req
        .header(reqwest::header::CONTENT_LENGTH, len)
        .body(body))
}

/// 把磁盘文件包装成 multipart 部件（流式，带长度）。
pub(crate) async fn stream_file_part(
    path: &str,
    file_name: &str,
) -> anyhow::Result<reqwest::multipart::Part> {
    let file = tokio::fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    Ok(reqwest::multipart::Part::stream_with_length(
        reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file)),
        len,
    )
    .file_name(file_name.to_string())
    .mime_str("application/pdf")?)
}

/// 从 ZIP 包中提取 markdown 和图片文件
fn extract_from_zip(zip_data: &[u8], output_dir: &str) -> (Option<String>, Vec<(String, Vec<u8>)>) {
    let mut markdown_content: Option<String> = None;
    let mut image_bytes: Vec<(String, Vec<u8>)> = Vec::new();

    if let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(zip_data)) {
        for i in 0..archive.len() {
            if let Ok(mut entry) = archive.by_index(i) {
                let name = entry.name().to_string();
                let lower = name.to_lowercase();
                let fname = Path::new(&name)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| name.clone());

                if lower.ends_with(".md") || lower.ends_with(".markdown") {
                    let mut buf = Vec::new();
                    if entry.read_to_end(&mut buf).is_ok() {
                        markdown_content = Some(String::from_utf8_lossy(&buf).to_string());
                    }
                } else if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")
                    || lower.ends_with(".gif") || lower.ends_with(".svg") || lower.ends_with(".webp")
                    || lower.ends_with(".bmp")
                {
                    let mut buf = Vec::new();
                    if entry.read_to_end(&mut buf).is_ok() {
                        let img_path = Path::new(output_dir).join(&fname);
                        if let Some(parent) = img_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&img_path, &buf);
                        image_bytes.push((fname, buf));
                    }
                }
            }
        }
    }

    (markdown_content, image_bytes)
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
                    let _ = zip_writer.write_all(data);
                }
            }
            let _ = zip_writer.finish();
        }
        Err(e) => {
            tracing::warn!("无法创建 images.zip: {}", e);
        }
    }
}

/// 一个解析分片。
///
/// - **本地切页**（首选）：`file_path` 是 mutool 切出来的临时 PDF，`page_range = None`、
///   `temporary = true`，解析完即删除；
/// - **按页码范围提交**（本地切页不可用时的退路）：`file_path` 是原文件，
///   `page_range = Some("1-100")`，`temporary = false`（**绝不能删原文件**）。
#[derive(Debug, Clone)]
pub struct ParseChunk {
    pub file_path: String,
    pub page_range: Option<String>,
    /// 这一片包含多少页（仅用于进度汇报）
    pub pages: i64,
    /// 是否是本地切页产生的临时文件（true = 用完可删）
    pub temporary: bool,
}

impl ParseChunk {
    /// 日志里的分片描述
    fn label(&self) -> String {
        match (&self.page_range, self.temporary) {
            (Some(range), _) => format!("页码 {}", range),
            (None, true) => format!("本地分片 {} 页", self.pages),
            (None, false) => format!("{} 页", self.pages),
        }
    }
}

/// 删掉本地切页产生的临时 PDF（`temporary = false` 时不动，避免误删原文件）
fn cleanup_split_file(chunk: &ParseChunk) {
    if chunk.temporary {
        let _ = std::fs::remove_file(&chunk.file_path);
    }
}

/// 规划分片来源：**优先本地切页**（mutool，总上传量 = 原文件大小），
/// 切页不可用时退回「整份原文件 + 页码范围」（要求该 API 支持页码参数）。
///
/// 返回空列表表示分不了片，调用方按「整份提交」继续。
pub async fn plan_chunks(
    src: &str,
    ranges: &[String],
    split_dir: &str,
    api_supports_page_range: bool,
) -> Vec<ParseChunk> {
    if ranges.is_empty() {
        return Vec::new();
    }
    match crate::services::pdf_split::split_pdf(src, ranges, split_dir).await {
        Ok(files) if files.len() == ranges.len() => files
            .into_iter()
            .zip(ranges.iter())
            .map(|(path, range)| ParseChunk {
                file_path: path,
                page_range: None,
                pages: pages_in_range(range),
                temporary: true,
            })
            .collect(),
        Ok(files) => {
            warn!(
                "本地切页数量不符（{} != {}），退回按页码范围提交",
                files.len(),
                ranges.len()
            );
            let _ = std::fs::remove_dir_all(split_dir);
            page_range_chunks(src, ranges, api_supports_page_range)
        }
        Err(e) => {
            warn!("本地切页不可用（{}），退回按页码范围提交", e);
            page_range_chunks(src, ranges, api_supports_page_range)
        }
    }
}

/// 退路：同一个原文件 + 每片各自的页码范围（每片都要重新上传整份文件）
fn page_range_chunks(
    src: &str,
    ranges: &[String],
    api_supports_page_range: bool,
) -> Vec<ParseChunk> {
    if !api_supports_page_range {
        warn!(
            "当前 MinerU 类型不支持页码范围，且本地切页不可用 → 只能整份提交（可能触发官方 API 页数上限）"
        );
        return Vec::new();
    }
    ranges
        .iter()
        .map(|range| ParseChunk {
            file_path: src.to_string(),
            page_range: Some(range.clone()),
            pages: pages_in_range(range),
            temporary: false,
        })
        .collect()
}

/// 按 `page_step` 页一片生成页码范围（1-based、含端点），如 250 页 / 100 → `["1-100","101-200","201-250"]`
///
/// `total_pages` 或 `page_step` 非正时返回空列表（调用方按「不分片」处理）。
pub fn plan_page_ranges(total_pages: i64, page_step: i64) -> Vec<String> {
    if total_pages <= 0 || page_step <= 0 {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = 1i64;
    while start <= total_pages {
        let end = (start + page_step - 1).min(total_pages);
        ranges.push(format!("{}-{}", start, end));
        start = end + 1;
    }
    ranges
}

/// 页码范围内包含多少页（`"101-200"` → 100；解析失败按 0 计，仅用于进度估算）
fn pages_in_range(range: &str) -> i64 {
    let range = range.trim();
    match range.split_once('-') {
        Some((a, b)) => match (a.trim().parse::<i64>(), b.trim().parse::<i64>()) {
            (Ok(a), Ok(b)) if b >= a => b - a + 1,
            _ => 0,
        },
        None => range.parse::<i64>().map(|_| 1).unwrap_or(0),
    }
}

/// 是否为图片文件名（与 [`extract_from_zip`] 认可的扩展名一致）
fn is_image_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// 合并各分片的结果目录为单一 Markdown + images.zip。
///
/// - Markdown 按分片顺序用空行拼接（分片顺序 = 页码范围顺序 = 文档页序）；
/// - 图片同名同内容只保留一份；同名**不同内容**时改名为 `chunkN__原名` 并同步改写
///   该分片正文里的引用（前端 `read_image` 只认 basename，所以只改文件名即可）。
fn merge_chunk_results(
    md_paths: &[String],
    output_dir: &str,
    doc_id: Option<i64>,
) -> anyhow::Result<String> {
    let out_dir = Path::new(output_dir);
    std::fs::create_dir_all(out_dir)?;

    let mut merged = String::new();
    // 已合并的图片：文件名 → 内容（用于判重与打包）
    let mut images: HashMap<String, Vec<u8>> = HashMap::new();

    for (idx, md_path) in md_paths.iter().enumerate() {
        let md_path = Path::new(md_path);
        let mut content = std::fs::read_to_string(md_path).unwrap_or_default();
        let chunk_dir = md_path.parent().unwrap_or(out_dir);

        let mut renames = 0usize;
        if let Ok(entries) = std::fs::read_dir(chunk_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                else {
                    continue;
                };
                if !is_image_name(&name) {
                    continue;
                }
                let Ok(data) = std::fs::read(&path) else {
                    continue;
                };
                match images.get(&name) {
                    // 同名同内容（重复图片）：只保留一份
                    Some(existing) if existing == &data => {}
                    None => {
                        images.insert(name, data);
                    }
                    // 同名不同内容：改名，避免后一片盖掉前一片的图
                    Some(_) => {
                        let renamed = format!("chunk{}__{}", idx + 1, name);
                        content = content.replace(&name, &renamed);
                        debug!("分片 {} 图片重名，{} → {}", idx + 1, name, renamed);
                        renames += 1;
                        images.insert(renamed, data);
                    }
                }
            }
        }
        if renames > 0 {
            debug!("分片 {} 共 {} 张重名图片已改名", idx + 1, renames);
        }

        let trimmed = content.trim_end();
        if trimmed.is_empty() {
            warn!("分片 {} 的 Markdown 为空，跳过", idx + 1);
            continue;
        }
        if !merged.is_empty() {
            merged.push_str("\n\n");
        }
        merged.push_str(trimmed);
    }

    if merged.is_empty() {
        anyhow::bail!("分片解析结果合并后 Markdown 为空");
    }

    if !images.is_empty() {
        let list: Vec<(String, Vec<u8>)> = images.into_iter().collect();
        info!("分片解析合并图片 {} 张", list.len());
        save_images_zip(output_dir, &list);
    }

    let md_filename = match doc_id {
        Some(id) => format!("{}.md", id),
        None => "output.md".to_string(),
    };
    let md_path = out_dir.join(&md_filename);
    // 合并后的 Markdown 必须是「图片与 images.zip 的同级目录」，
    // finish_parse_result 就按这个约定去找 images.zip。
    std::fs::write(&md_path, merged)?;
    Ok(md_path.to_string_lossy().to_string())
}

/// 解析 URL：将相对路径拼接到 base_url 上
fn resolve_url(base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("{}{}", base.trim_end_matches('/'), url)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), url)
    }
}

/// 判断两个 URL 是否同源
fn is_same_origin(url_a: &str, url_b: &str) -> bool {
    let a_parsed = url::Url::parse(url_a);
    let b_parsed = url::Url::parse(url_b);
    match (a_parsed, b_parsed) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}