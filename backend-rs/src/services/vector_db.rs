//! 向量数据库抽象层。

use serde_json::Value;

use crate::services::qdrant::QdrantClient;

/// 向量数据库适配器 trait
#[async_trait::async_trait]
pub trait VectorDBAdapter: Send + Sync {
    async fn ensure_collection(&self, collection_name: &str) -> anyhow::Result<()>;
    async fn index_document(
        &self,
        doc_id: i64,
        user_id: i64,
        chunks: &[String],
        embeddings: &[Vec<f64>],
        metadata: &Value,
    ) -> anyhow::Result<String>;
    async fn search_similar(
        &self,
        user_id: i64,
        query_embedding: &[f64],
        limit: usize,
    ) -> anyhow::Result<Vec<Value>>;
    async fn delete_document(&self, user_id: i64, doc_id: i64) -> anyhow::Result<()>;
    async fn list_collections(&self) -> anyhow::Result<Vec<String>>;
    async fn delete_collection(&self, collection_name: &str) -> anyhow::Result<bool>;
    async fn get_point(&self, point_id: &str, collection_name: Option<&str>) -> anyhow::Result<Option<Value>>;
    async fn health_check(&self) -> anyhow::Result<Value>;
    fn get_collection_name(&self, user_id: i64) -> String;
}

/// Qdrant 适配器
pub struct QdrantAdapter {
    client: QdrantClient,
    base_collection: String,
    vector_size: usize,
}

impl QdrantAdapter {
    pub fn new(db_config: &Value) -> Self {
        let url = db_config["url"].as_str()
            .unwrap_or("http://127.0.0.1:6333")
            .trim_end_matches('/')
            .to_string();
        let collection = db_config["collection"].as_str()
            .unwrap_or("documents")
            .to_string();
        let vector_size = get_vector_size(db_config);
        Self {
            client: QdrantClient::new(&url),
            base_collection: collection,
            vector_size,
        }
    }
}

#[async_trait::async_trait]
impl VectorDBAdapter for QdrantAdapter {
    async fn ensure_collection(&self, collection_name: &str) -> anyhow::Result<()> {
        let collections = self.client.list_collections().await
            .map_err(|e| anyhow::anyhow!(e))?;
        if !collections.contains(&collection_name.to_string()) {
            self.client.create_collection(collection_name, self.vector_size).await
                .map_err(|e| anyhow::anyhow!(e))?;
        }
        Ok(())
    }

    async fn index_document(
        &self,
        doc_id: i64,
        user_id: i64,
        chunks: &[String],
        embeddings: &[Vec<f64>],
        metadata: &Value,
    ) -> anyhow::Result<String> {
        let collection_name = self.get_collection_name(user_id);
        self.ensure_collection(&collection_name).await?;
        self.client.delete_document_points(&collection_name, doc_id).await
            .map_err(|e| anyhow::anyhow!(e))?;

        let mut points = Vec::new();
        for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            if embedding.is_empty() {
                continue;
            }
            let point_id = uuid::Uuid::new_v4().to_string();
            let point = serde_json::json!({
                "id": point_id,
                "vector": embedding,
                "payload": {
                    "text": chunk,
                    "metadata": {
                        "document_id": doc_id,
                        "chunk_index": i,
                        "title": metadata.get("title").unwrap_or(&serde_json::Value::String("".to_string())),
                        "authors": metadata.get("authors").unwrap_or(&serde_json::Value::Array(vec![])),
                        "year": metadata.get("year"),
                        "doc_type": metadata.get("type").unwrap_or(&serde_json::Value::String("".to_string())),
                        "keywords": metadata.get("keywords").unwrap_or(&serde_json::Value::Array(vec![])),
                    }
                }
            });
            points.push(point);
        }

        self.client.upsert_points(&collection_name, &points).await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(collection_name)
    }

    async fn search_similar(
        &self,
        user_id: i64,
        query_embedding: &[f64],
        limit: usize,
    ) -> anyhow::Result<Vec<Value>> {
        let collection_name = self.get_collection_name(user_id);
        let vec_f32: Vec<f32> = query_embedding.iter().map(|&x| x as f32).collect();
        let results = self.client.search(&collection_name, &vec_f32, limit).await
            .map_err(|e| anyhow::anyhow!(e))?;
        let mut formatted = Vec::new();
        for hit in &results {
            formatted.push(serde_json::json!({
                "score": hit["score"],
                "document_id": hit["payload"]["metadata"]["document_id"],
                "chunk_text": hit["payload"]["text"],
                "title": hit["payload"]["metadata"]["title"],
            }));
        }
        Ok(formatted)
    }

    async fn delete_document(&self, user_id: i64, doc_id: i64) -> anyhow::Result<()> {
        let collection_name = self.get_collection_name(user_id);
        self.client.delete_document_points(&collection_name, doc_id).await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }

    async fn list_collections(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.client.list_collections().await.map_err(|e| anyhow::anyhow!(e))?)
    }

    async fn delete_collection(&self, collection_name: &str) -> anyhow::Result<bool> {
        self.client.delete_collection(collection_name).await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(true)
    }

    async fn get_point(&self, point_id: &str, collection_name: Option<&str>) -> anyhow::Result<Option<Value>> {
        let col = collection_name.unwrap_or(&self.base_collection);
        Ok(self.client.get_point(col, point_id).await.map_err(|e| anyhow::anyhow!(e))?)
    }

    async fn health_check(&self) -> anyhow::Result<Value> {
        Ok(self.client.health_check().await.map_err(|e| anyhow::anyhow!(e))?)
    }

    fn get_collection_name(&self, user_id: i64) -> String {
        self.base_collection.clone()
    }
}

fn get_vector_size(db_config: &Value) -> usize {
    let model = db_config["embedding"]["model"].as_str().unwrap_or("");
    match model {
        "nomic-embed-text" | "nomic-ai/nomic-embed-text-v1.5" | "BAAI/bge-base-en-v1.5" | "BAAI/bge-base-zh-v1.5" | "sentence-transformers/all-mpnet-base-v2" => 768,
        "BAAI/bge-small-en-v1.5" | "sentence-transformers/all-MiniLM-L6-v2" | "all-minilm" => 384,
        "BAAI/bge-large-en-v1.5" | "BAAI/bge-large-zh-v1.5" | "mxbai-embed-large" | "bge-m3" => 1024,
        _ => 768,
    }
}

/// 获取向量数据库适配器
pub fn get_vector_db(db_id: Option<&str>) -> anyhow::Result<Box<dyn VectorDBAdapter>> {
    let config_manager = crate::config_manager::ConfigManager::new();
    let db_config = if let Some(id) = db_id {
        config_manager.get_vector_db_by_id(id)
    } else {
        config_manager.get_active_vector_db()
    };

    let db_config = db_config.ok_or_else(|| anyhow::anyhow!("没有可用的向量数据库配置"))?;
    let db_type = db_config["type"].as_str().unwrap_or("qdrant");

    match db_type {
        "qdrant" => Ok(Box::new(QdrantAdapter::new(&db_config))),
        "milvus" => Ok(Box::new(MilvusAdapter::new(&db_config))),
        "chroma" => Ok(Box::new(ChromaAdapter::new(&db_config))),
        _ => anyhow::bail!("不支持的向量数据库类型: {}", db_type),
    }
}

/// Milvus 适配器（骨架）
pub struct MilvusAdapter {
    url: String,
    base_collection: String,
}

impl MilvusAdapter {
    pub fn new(db_config: &Value) -> Self {
        Self {
            url: db_config["url"].as_str().unwrap_or("http://127.0.0.1:19530").to_string(),
            base_collection: db_config["collection"].as_str().unwrap_or("documents").to_string(),
        }
    }
}

#[async_trait::async_trait]
impl VectorDBAdapter for MilvusAdapter {
    async fn ensure_collection(&self, _collection_name: &str) -> anyhow::Result<()> {
        anyhow::bail!("Milvus 适配器尚未实现")
    }
    async fn index_document(&self, _doc_id: i64, _user_id: i64, _chunks: &[String], _embeddings: &[Vec<f64>], _metadata: &Value) -> anyhow::Result<String> {
        anyhow::bail!("Milvus 适配器尚未实现")
    }
    async fn search_similar(&self, _user_id: i64, _query_embedding: &[f64], _limit: usize) -> anyhow::Result<Vec<Value>> {
        anyhow::bail!("Milvus 适配器尚未实现")
    }
    async fn delete_document(&self, _user_id: i64, _doc_id: i64) -> anyhow::Result<()> {
        anyhow::bail!("Milvus 适配器尚未实现")
    }
    async fn list_collections(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec![])
    }
    async fn delete_collection(&self, _collection_name: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn get_point(&self, _point_id: &str, _collection_name: Option<&str>) -> anyhow::Result<Option<Value>> {
        Ok(None)
    }
    async fn health_check(&self) -> anyhow::Result<Value> {
        Ok(serde_json::json!({"status": "not_implemented", "url": self.url}))
    }
    fn get_collection_name(&self, _user_id: i64) -> String {
        self.base_collection.clone()
    }
}

/// Chroma 适配器（骨架）
pub struct ChromaAdapter {
    url: String,
    base_collection: String,
}

impl ChromaAdapter {
    pub fn new(db_config: &Value) -> Self {
        Self {
            url: db_config["url"].as_str().unwrap_or("http://127.0.0.1:8000").to_string(),
            base_collection: db_config["collection"].as_str().unwrap_or("documents").to_string(),
        }
    }
}

#[async_trait::async_trait]
impl VectorDBAdapter for ChromaAdapter {
    async fn ensure_collection(&self, _collection_name: &str) -> anyhow::Result<()> {
        anyhow::bail!("Chroma 适配器尚未实现")
    }
    async fn index_document(&self, _doc_id: i64, _user_id: i64, _chunks: &[String], _embeddings: &[Vec<f64>], _metadata: &Value) -> anyhow::Result<String> {
        anyhow::bail!("Chroma 适配器尚未实现")
    }
    async fn search_similar(&self, _user_id: i64, _query_embedding: &[f64], _limit: usize) -> anyhow::Result<Vec<Value>> {
        anyhow::bail!("Chroma 适配器尚未实现")
    }
    async fn delete_document(&self, _user_id: i64, _doc_id: i64) -> anyhow::Result<()> {
        anyhow::bail!("Chroma 适配器尚未实现")
    }
    async fn list_collections(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec![])
    }
    async fn delete_collection(&self, _collection_name: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn get_point(&self, _point_id: &str, _collection_name: Option<&str>) -> anyhow::Result<Option<Value>> {
        Ok(None)
    }
    async fn health_check(&self) -> anyhow::Result<Value> {
        Ok(serde_json::json!({"status": "not_implemented", "url": self.url}))
    }
    fn get_collection_name(&self, _user_id: i64) -> String {
        self.base_collection.clone()
    }
}

/// 文本分块（按段落）
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut chunks = Vec::new();
    let mut current = String::new();

    for para in paragraphs {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if current.len() + para.len() < chunk_size {
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(para);
        } else {
            if !current.is_empty() {
                chunks.push(current.clone());
            }
            current = para.to_string();
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    // Add overlap
    if overlap > 0 && chunks.len() > 1 {
        let mut overlapped = vec![chunks[0].clone()];
        for i in 1..chunks.len() {
            let prev = &chunks[i - 1];
            let overlap_start = if prev.len() > overlap {
                let space_idx = prev[prev.len() - overlap..].find(' ');
                match space_idx {
                    Some(idx) => prev.len() - overlap + idx + 1,
                    None => prev.len() - overlap,
                }
            } else {
                0
            };
            overlapped.push(format!("{} {}", &prev[overlap_start..], chunks[i]));
        }
        return overlapped;
    }

    chunks
}

/// 按 Markdown 结构分块
pub fn chunk_text_by_markdown(text: &str, chunk_size: usize, min_chunk_size: usize) -> Vec<String> {
    use regex::Regex;
    if text.trim().is_empty() {
        return Vec::new();
    }

    let header_pattern = Regex::new(r"^(#{1,6})\s+(.+)$").unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_header = String::new();
    let mut current_content: Vec<&str> = Vec::new();

    for line in &lines {
        if header_pattern.is_match(line) {
            if !current_content.is_empty() {
                sections.push((current_header.clone(), current_content.join("\n").trim().to_string()));
                current_content = Vec::new();
            }
            current_header = line.to_string();
        } else {
            current_content.push(line);
        }
    }
    if !current_content.is_empty() {
        sections.push((current_header.clone(), current_content.join("\n").trim().to_string()));
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut current_parts: Vec<String> = Vec::new();
    let mut current_size = 0;

    for (header, content) in &sections {
        let section_text = if header.is_empty() {
            content.clone()
        } else {
            format!("{}\n{}", header, content)
        };

        if section_text.len() > chunk_size {
            if !current_parts.is_empty() {
                chunks.push(current_parts.join("\n\n"));
                current_parts = Vec::new();
                current_size = 0;
            }
            // Split long section by paragraphs
            let paragraphs: Vec<&str> = content.split("\n\n").collect();
            let mut sub = if header.is_empty() { String::new() } else { format!("{}\n", header) };
            for para in paragraphs {
                let para = para.trim();
                if para.is_empty() { continue; }
                if sub.len() + para.len() + 2 <= chunk_size {
                    if !sub.trim().is_empty() { sub.push_str("\n\n"); }
                    sub.push_str(para);
                } else {
                    if sub.trim().len() >= min_chunk_size {
                        chunks.push(sub.trim().to_string());
                    }
                    sub = if header.is_empty() { format!("{}", para) } else { format!("{}\n\n{}", header, para) };
                }
            }
            if sub.trim().len() >= min_chunk_size {
                current_parts = vec![sub.trim().to_string()];
                current_size = sub.trim().len();
            }
        } else {
            current_parts.push(section_text.clone());
            current_size += section_text.len();
            if current_size >= chunk_size {
                chunks.push(current_parts.join("\n\n"));
                current_parts = Vec::new();
                current_size = 0;
            }
        }
    }

    if !current_parts.is_empty() {
        let final_text = current_parts.join("\n\n");
        if final_text.len() < min_chunk_size && !chunks.is_empty() {
            let last = chunks.pop().unwrap();
            chunks.push(format!("{}\n\n{}", last, final_text));
        } else {
            chunks.push(final_text);
        }
    }

    chunks
}