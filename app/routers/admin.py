import os

import httpx
from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.orm import Session
from sqlalchemy import text

from app.config import get_settings
from app.database import get_db, engine
from app.models import Document, DocStatus
from app.services.qdrant import get_qdrant_client, get_point, list_collections, delete_collection
from app.utils import calculate_file_hash

router = APIRouter(prefix="/assist/api/admin", tags=["admin"])
settings = get_settings()


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
    """Check status of all external services (MinerU, Ollama, Qdrant)."""
    result = {
        "mineru": {"status": "unknown", "url": settings.mineru_url},
        "ollama": {"status": "unknown", "url": settings.ollama_url},
        "qdrant": {"status": "unknown", "url": settings.qdrant_url},
    }

    # Check MinerU
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            response = await client.get(f"{settings.mineru_url}/health")
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
            response = await client.get(f"{settings.ollama_url}/api/tags")
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

    # Check Qdrant
    try:
        client = get_qdrant_client()
        collections = client.get_collections().collections
        result["qdrant"]["status"] = "connected"
        result["qdrant"]["collections"] = [c.name for c in collections]
    except Exception as e:
        result["qdrant"]["status"] = "disconnected"
        result["qdrant"]["error"] = str(e)

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
        if not doc.file_path or not os.path.exists(doc.file_path):
            skipped += 1
            continue
        try:
            doc.file_hash = calculate_file_hash(doc.file_path)
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
