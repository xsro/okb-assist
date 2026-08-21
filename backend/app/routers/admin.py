import os

import httpx
from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.orm import Session
from sqlalchemy import text

from app.config import get_settings

settings = get_settings()
from app.config_manager import get_config
from app.database import get_db, engine
from app.models import Document, DocStatus
from app.services.qdrant import get_qdrant_client, get_point, list_collections, delete_collection
from app.services.vector_db import get_vector_db
from app.utils import calculate_file_hash
from app.paths import get_pdf_path

router = APIRouter(prefix="/assist/api/admin", tags=["admin"])


@router.get("/stats")
def get_stats(db: Session = Depends(get_db)):
    doc_count = db.query(Document).count()

    # Count by status
    status_counts = {}
    for s in DocStatus:
        count = db.query(Document).filter(Document.status == s).count()
        status_counts[s.value] = count

    # Check Qdrant connection
    qdrant_status = "connected"
    qdrant_collections = []
    try:
        client = get_qdrant_client()
        collections = client.get_collections().collections
        qdrant_collections = [c.name for c in collections]
    except Exception as e:
        qdrant_status = f"error: {str(e)}"

    return {
        "documents": doc_count,
        "status_counts": status_counts,
        "qdrant_status": qdrant_status,
        "qdrant_collections": qdrant_collections,
    }


@router.get("/services/status")
async def get_services_status():
    """Check status of all external services (MinerU, Ollama, VectorDBs)."""
    cfg = get_config()
    mineru_url = settings.mineru_url  # 已自动去除尾部斜杠
    mineru_key = settings.mineru_key
    ollama_url = settings.ollama_url
    fastembed_url = settings.fastembed_url

    result = {
        "mineru": {"status": "unknown", "url": mineru_url},
        "ollama": {"status": "unknown", "url": ollama_url},
        "fastembed": {"status": "unknown", "url": fastembed_url},
    }

    # Check MinerU
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            headers = {}
            if mineru_key:
                headers["Authorization"] = f"Bearer {mineru_key}"
            response = await client.get(f"{mineru_url}/health", headers=headers)
            if response.status_code == 200:
                health = response.json()
                result["mineru"]["status"] = "connected"
                result["mineru"]["version"] = health.get("version", "unknown")
                result["mineru"]["queued_tasks"] = health.get("queued_tasks", 0)
                result["mineru"]["processing_tasks"] = health.get("processing_tasks", 0)
                result["mineru"]["completed_tasks"] = health.get("completed_tasks", 0)
                result["mineru"]["failed_tasks"] = health.get("failed_tasks", 0)
                result["mineru"]["max_concurrent"] = health.get("max_concurrent_requests", 0)
            else:
                result["mineru"]["status"] = "error"
                result["mineru"]["error"] = f"HTTP {response.status_code}"
    except Exception as e:
        result["mineru"]["status"] = "disconnected"
        result["mineru"]["error"] = str(e)

    # Check Ollama
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            response = await client.get(f"{ollama_url}/api/tags")
            if response.status_code == 200:
                models = response.json().get("models", [])
                result["ollama"]["status"] = "connected"
                result["ollama"]["models"] = [m["name"] for m in models]
            else:
                result["ollama"]["status"] = "error"
                result["ollama"]["error"] = f"HTTP {response.status_code}"
    except Exception as e:
        result["ollama"]["status"] = "disconnected"
        result["ollama"]["error"] = str(e)

    # Check FastEmbed
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            response = await client.get(f"{fastembed_url}/health")
            if response.status_code == 200:
                health = response.json()
                result["fastembed"]["status"] = "connected"
                result["fastembed"]["default_model"] = health.get("default_model", "")
                result["fastembed"]["loaded_models"] = health.get("loaded_models", [])
            else:
                result["fastembed"]["status"] = "error"
                result["fastembed"]["error"] = f"HTTP {response.status_code}"
    except Exception as e:
        result["fastembed"]["status"] = "disconnected"
        result["fastembed"]["error"] = str(e)

    # Check all configured vector databases
    result["vector_dbs"] = []
    for db_cfg in cfg.get("vector_dbs", []):
        db_info = {
            "id": db_cfg.get("id"),
            "name": db_cfg.get("name"),
            "type": db_cfg.get("type"),
            "enabled": db_cfg.get("enabled"),
            "url": db_cfg.get("url"),
        }
        if db_cfg.get("enabled"):
            try:
                adapter = get_vector_db(db_cfg["id"])
                health = await adapter.health_check()
                db_info.update(health)
            except Exception as e:
                db_info["status"] = "error"
                db_info["error"] = str(e)
        else:
            db_info["status"] = "disabled"
        result["vector_dbs"].append(db_info)

    # 兼容旧代码：保留顶层 qdrant 字段
    qdrant_db = next((d for d in result["vector_dbs"] if d["type"] == "qdrant" and d.get("enabled")), None)
    if qdrant_db:
        result["qdrant"] = qdrant_db
    else:
        result["qdrant"] = {"status": "not_configured"}

    return result


@router.get("/mineru/tasks")
async def get_mineru_tasks():
    """Get MinerU task status."""
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            response = await client.get(f"{settings.mineru_url}/tasks")
            if response.status_code == 200:
                return response.json()
            else:
                return {"error": f"HTTP {response.status_code}", "detail": response.text}
    except Exception as e:
        return {"error": str(e)}


@router.get("/qdrant/collections")
def get_collections():
    """List all Qdrant collections."""
    collections = list_collections()
    return {"collections": collections}


@router.get("/qdrant/point/{point_id}")
def get_point_data(point_id: str, collection: str = None):
    """Get point data from Qdrant by ID."""
    result = get_point(point_id, collection)
    if result is None:
        raise HTTPException(status_code=404, detail="Point not found")
    if "error" in result:
        raise HTTPException(status_code=500, detail=result["error"])
    return result


@router.post("/db/migrate")
def run_database_migration():
    """Run database migrations to add missing columns."""
    migrations_applied = []

    with engine.connect() as conn:
        # Check if mineru_task_id column exists
        try:
            result = conn.execute(text("SELECT mineru_task_id FROM documents LIMIT 1"))
            migrations_applied.append("mineru_task_id column already exists")
        except Exception:
            # Column doesn't exist, add it
            conn.execute(text("ALTER TABLE documents ADD COLUMN mineru_task_id VARCHAR(100)"))
            conn.commit()
            migrations_applied.append("Added mineru_task_id column")

        # Check if vector_db_id column exists
        try:
            result = conn.execute(text("SELECT vector_db_id FROM documents LIMIT 1"))
            migrations_applied.append("vector_db_id column already exists")
        except Exception:
            conn.execute(text("ALTER TABLE documents ADD COLUMN vector_db_id VARCHAR(50)"))
            conn.commit()
            migrations_applied.append("Added vector_db_id column")

    return {
        "detail": "Migration completed",
        "migrations": migrations_applied,
    }


@router.post("/recalculate-hashes")
def recalculate_hashes(db: Session = Depends(get_db)):
    """重新计算所有文档的文件哈希。"""
    docs = db.query(Document).all()
    updated = 0
    skipped = 0
    errors = []

    for doc in docs:
        # 源 PDF 路径由 system.json 推导
        abs_file_path = get_pdf_path(doc.id)
        if not os.path.exists(abs_file_path):
            skipped += 1
            continue

        try:
            doc.file_hash = calculate_file_hash(abs_file_path)
            updated += 1
        except Exception as e:
            errors.append({"id": doc.id, "error": str(e)})

    db.commit()

    return {
        "detail": f"哈希重算完成: 更新 {updated} 条，跳过 {skipped} 条",
        "updated": updated,
        "skipped": skipped,
        "errors": errors,
    }


@router.post("/reset-index")
def reset_index(db: Session = Depends(get_db)):
    """重置索引：删除 Qdrant 集合并将所有已索引文档重置为已提取状态。"""
    # Find all indexed documents
    indexed_docs = db.query(Document).filter(Document.status == DocStatus.indexed).all()
    count = len(indexed_docs)

    # Collect unique collection names to delete
    collections_to_delete = set()
    for doc in indexed_docs:
        if doc.qdrant_collection:
            collections_to_delete.add(doc.qdrant_collection)

    # Delete Qdrant collections
    deleted_collections = []
    for col_name in collections_to_delete:
        if delete_collection(col_name):
            deleted_collections.append(col_name)

    # Reset all indexed documents to meta_done
    db.query(Document).filter(Document.status == DocStatus.indexed).update(
        {
            Document.status: DocStatus.meta_done,
            Document.qdrant_collection: None,
            Document.status_message: None,
            Document.progress: 0.0,
        }
    )
    db.commit()

    return {
        "detail": f"索引重置完成: {count} 个文档已重置为已提取状态",
        "reset_count": count,
        "deleted_collections": deleted_collections,
    }
