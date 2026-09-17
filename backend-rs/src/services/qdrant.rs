//! Qdrant 向量数据库 HTTP 客户端（纯 HTTP 实现，无官方 Rust SDK）。

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

/// Qdrant 客户端
pub struct QdrantClient {
    base_url: String,
    http: reqwest::Client,
}

impl QdrantClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// 获取集合列表
    pub async fn list_collections(&self) -> Result<Vec<String>, String> {
        let url = format!("{}/collections", self.base_url);
        let resp = self.http.get(&url).send().await
            .map_err(|e| format!("Qdrant request failed: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("Qdrant error: HTTP {}", resp.status()));
        }
        let data: Value = resp.json().await
            .map_err(|e| format!("Qdrant parse error: {}", e))?;
        let collections = data["result"]["collections"]
            .as_array()
            .map(|arr| arr.iter()
                .filter_map(|c| c["name"].as_str().map(String::from))
                .collect())
            .unwrap_or_default();
        Ok(collections)
    }

    /// 创建集合
    pub async fn create_collection(&self, collection_name: &str, vector_size: usize) -> Result<(), String> {
        let url = format!("{}/collections/{}", self.base_url, collection_name);
        let body = json!({
            "vectors": {
                "size": vector_size,
                "distance": "Cosine"
            }
        });
        let resp = self.http.put(&url)
            .json(&body)
            .send().await
            .map_err(|e| format!("Qdrant create collection failed: {}", e))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Qdrant create collection error: {}", text));
        }
        Ok(())
    }

    /// 上点（upsert）
    pub async fn upsert_points(
        &self,
        collection_name: &str,
        points: &[Value],
    ) -> Result<(), String> {
        let url = format!("{}/collections/{}/points", self.base_url, collection_name);
        let body = json!({ "points": points });
        let resp = self.http.put(&url)
            .json(&body)
            .send().await
            .map_err(|e| format!("Qdrant upsert failed: {}", e))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Qdrant upsert error: {}", text));
        }
        Ok(())
    }

    /// 搜索向量
    pub async fn search(
        &self,
        collection_name: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        let url = format!("{}/collections/{}/points/search", self.base_url, collection_name);
        let body = json!({
            "vector": query_vector,
            "limit": limit,
            "with_payload": true,
        });
        let resp = self.http.post(&url)
            .json(&body)
            .send().await
            .map_err(|e| format!("Qdrant search failed: {}", e))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Qdrant search error: {}", text));
        }
        let data: Value = resp.json().await
            .map_err(|e| format!("Qdrant parse error: {}", e))?;
        let points = data["result"]["points"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(points)
    }

    /// 删除文档的所有点
    pub async fn delete_document_points(
        &self,
        collection_name: &str,
        doc_id: i64,
    ) -> Result<(), String> {
        let url = format!("{}/collections/{}/points", self.base_url, collection_name);
        let body = json!({
            "filter": {
                "must": [
                    {
                        "key": "metadata.document_id",
                        "match": { "value": doc_id }
                    }
                ]
            }
        });
        let resp = self.http.post(format!("{}/delete", url))
            .json(&body)
            .send().await
            .map_err(|e| format!("Qdrant delete failed: {}", e))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Qdrant delete error: {}", text));
        }
        Ok(())
    }

    /// 删除集合
    pub async fn delete_collection(&self, collection_name: &str) -> Result<(), String> {
        let url = format!("{}/collections/{}", self.base_url, collection_name);
        let resp = self.http.delete(&url).send().await
            .map_err(|e| format!("Qdrant delete collection failed: {}", e))?;
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Qdrant delete collection error: {}", text));
        }
        Ok(())
    }

    /// 获取点数据
    pub async fn get_point(
        &self,
        collection_name: &str,
        point_id: &str,
    ) -> Result<Option<Value>, String> {
        let url = format!("{}/collections/{}/points/{}", self.base_url, collection_name, point_id);
        let resp = self.http.get(&url).send().await
            .map_err(|e| format!("Qdrant get point failed: {}", e))?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Qdrant get point error: {}", text));
        }
        let data: Value = resp.json().await
            .map_err(|e| format!("Qdrant parse error: {}", e))?;
        Ok(Some(data["result"].clone()))
    }

    /// 健康检查
    pub async fn health_check(&self) -> Result<Value, String> {
        let url = format!("{}/collections", self.base_url);
        let resp = self.http.get(&url).send().await
            .map_err(|e| format!("Qdrant health check failed: {}", e))?;
        if resp.status().is_success() {
            let data: Value = resp.json().await
                .map_err(|e| format!("Qdrant parse error: {}", e))?;
            let collections: Vec<String> = data["result"]["collections"]
                .as_array()
                .map(|arr| arr.iter()
                    .filter_map(|c| c["name"].as_str().map(String::from))
                    .collect())
                .unwrap_or_default();
            Ok(json!({
                "status": "connected",
                "url": self.base_url,
                "collections": collections,
            }))
        } else {
            Ok(json!({
                "status": "disconnected",
                "url": self.base_url,
                "error": format!("HTTP {}", resp.status()),
            }))
        }
    }
}

/// 生成 Qdrant 点 ID
pub fn generate_point_id() -> String {
    Uuid::new_v4().to_string()
}

/// 将 f64 向量转换为 f32（Qdrant 使用 f32）
pub fn to_f32(vec: &[f64]) -> Vec<f32> {
    vec.iter().map(|&x| x as f32).collect()
}