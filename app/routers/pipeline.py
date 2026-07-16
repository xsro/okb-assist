import json
import os
import asyncio
from datetime import datetime, timezone
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, BackgroundTasks
from sqlalchemy.orm import Session

from app.config import get_settings
from app.config_manager import get_vector_db_by_id
from app.database import get_db, SessionLocal
from app.models import Document, DocStatus, DocumentVectorIndex, IndexStatus
from app.services.mineru import parse_pdf, submit_parse_task, poll_task, get_task_result, check_task_status
from app.services.ollama import extract_metadata, get_embedding
from app.services.qdrant import index_document, delete_document_points
from app.utils import to_absolute_path

router = APIRouter(prefix="/assist/api/pipeline", tags=["pipeline"])


# ── Zotero 标准类型映射 ──
_TYPE_NORMALIZE = {
    "article": "journalArticle", "journalarticle": "journalArticle", "journal article": "journalArticle",
    "conference": "conferencePaper", "conferencepaper": "conferencePaper", "conference paper": "conferencePaper",
    "inproceedings": "conferencePaper", "conference proceedings": "conferencePaper",
    "thesis": "thesis", "dissertation": "thesis", "硕士学位论文": "thesis", "博士学位论文": "thesis",
    "mastersthesis": "thesis", "phdthesis": "thesis",
    "book": "book", "booksection": "bookSection", "book section": "bookSection",
    "preprint": "preprint", "report": "report", "webpage": "webpage", "document": "document",
    "presentation": "presentation", "manuscript": "manuscript", "patent": "patent", "review": "review",
}


def _normalize_doc_type(raw: str) -> str:
    """将任意 doc_type 标准化为 Zotero 类型名。"""
    if not raw:
        return ""
    raw = raw.strip()
    return _TYPE_NORMALIZE.get(raw) or _TYPE_NORMALIZE.get(raw.lower()) or "document"

QDRANT_USER_ID = 0  # Default user ID for Qdrant since no auth

# Concurrency limiting
settings = get_settings()
MAX_CONCURRENT_TASKS = getattr(settings, 'max_concurrent_tasks', 3)
_task_semaphore = asyncio.Semaphore(MAX_CONCURRENT_TASKS)
_running_tasks = 0
_batch_paused = False  # Flag to pause batch processing

# Task tracking - 记录正在运行的任务
_active_tasks: dict[int, dict] = {}  # doc_id -> {task_type, started_at, status_message}

# Batch progress tracking - 批量任务整体进度
_batch_progress: dict = {
    "active": False,        # 是否有批量任务在运行
    "stage": "",            # 当前阶段: index / parse / extract
    "vector_db_id": "",     # 索引目标数据库
    "total": 0,             # 总文档数
    "processed": 0,         # 已处理数
    "current_batch": 0,     # 当前第几批
    "total_batches": 0,     # 总批数
    "batch_size": 0,        # 每批大小
    "pause_seconds": 0,     # 批间暂停秒数
    "started_at": None,     # 开始时间
    "errors": 0,            # 失败数
}


def _get_running_tasks() -> int:
    """Get number of currently running tasks."""
    return _running_tasks


def _track_task_start(doc_id: int, task_type: str):
    """记录任务开始"""
    _active_tasks[doc_id] = {
        "task_type": task_type,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "status_message": "正在处理..."
    }


def _track_task_update(doc_id: int, status_message: str):
    """更新任务状态消息"""
    if doc_id in _active_tasks:
        _active_tasks[doc_id]["status_message"] = status_message


def _track_task_end(doc_id: int):
    """记录任务结束"""
    _active_tasks.pop(doc_id, None)


async def _with_semaphore(coro):
    """Run a coroutine with semaphore limiting."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        try:
            return await coro
        finally:
            _running_tasks -= 1


def _update_doc_status(doc_id: int, status: DocStatus, message: str = None, progress: float = None):
    """Update document status in database."""
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if doc:
            doc.status = status
            if message is not None:
                doc.status_message = message
            if progress is not None:
                doc.progress = progress
            db.commit()
    finally:
        db.close()


async def _run_parse(doc_id: int, file_path: str):
    """Background task for parsing PDF."""
    await _with_semaphore(_do_parse_impl(doc_id, file_path))


async def _do_parse_impl(doc_id: int, file_path: str):
    """Parse implementation. Supports resuming from a previously timed-out MinerU task."""
    try:
        _update_doc_status(doc_id, DocStatus.parsing, "正在解析 PDF...", 10)

        # 将相对路径转换为绝对路径
        abs_file_path = to_absolute_path(file_path)

        # Save generated files in uploads/{doc_id}/
        output_dir = os.path.join(settings.uploads_folder, str(doc_id))
        os.makedirs(output_dir, exist_ok=True)

        # Check if there's an existing MinerU task (from a previous timeout)
        existing_task_id = None
        db = SessionLocal()
        try:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if doc and doc.mineru_task_id:
                existing_task_id = doc.mineru_task_id
        finally:
            db.close()

        task_id = None
        if existing_task_id:
            # Check if the existing task has completed
            _update_doc_status(doc_id, DocStatus.parsing, "正在检查之前的解析任务...", 15)
            status_result = await check_task_status(existing_task_id)
            status = status_result.get("status")

            if status == "completed":
                # Previous task finished, just get the result
                _update_doc_status(doc_id, DocStatus.parsing, "之前的任务已完成，正在获取结果...", 50)
                md_path = await get_task_result(existing_task_id, output_dir, doc_id)
                _save_parse_result(doc_id, md_path)
                return
            elif status == "failed":
                # Previous task failed, clear it and submit a new one
                _update_doc_status(doc_id, DocStatus.parsing, "之前的任务失败，重新提交...", 10)
                _clear_mineru_task_id(doc_id)
                existing_task_id = None
            elif status in ("processing", "pending"):
                # Previous task still running, resume polling
                _update_doc_status(doc_id, DocStatus.parsing, "正在等待之前的解析任务完成...", 20)
                try:
                    await poll_task(existing_task_id)
                    md_path = await get_task_result(existing_task_id, output_dir, doc_id)
                    _save_parse_result(doc_id, md_path)
                    return
                except Exception as e:
                    if "timed out" in str(e).lower():
                        # Still timed out, keep the task_id for next attempt
                        _update_doc_status(doc_id, DocStatus.error, f"解析失败: {str(e)}")
                        return
                    else:
                        # Other error, clear and retry
                        _clear_mineru_task_id(doc_id)
                        existing_task_id = None
            else:
                # Unknown status, clear and submit new
                _clear_mineru_task_id(doc_id)
                existing_task_id = None

        # Submit a new task (使用绝对路径)
        task_id = await submit_parse_task(abs_file_path)

        # Save the task_id to database for potential resume
        _save_mineru_task_id(doc_id, task_id)

        _update_doc_status(doc_id, DocStatus.parsing, "正在解析 PDF，已提交任务...", 20)

        # Poll for completion
        try:
            await poll_task(task_id)
        except Exception as e:
            if "timed out" in str(e).lower():
                # Timeout - task_id is already saved, can be resumed later
                _update_doc_status(doc_id, DocStatus.error, f"解析失败: {str(e)}")
                return
            raise

        # Get result
        _update_doc_status(doc_id, DocStatus.parsing, "正在获取解析结果...", 80)
        md_path = await get_task_result(task_id, output_dir, doc_id)

        _save_parse_result(doc_id, md_path)

    except Exception as e:
        _update_doc_status(doc_id, DocStatus.error, f"解析失败: {str(e)}")


def _save_mineru_task_id(doc_id: int, task_id: str):
    """Save MinerU task ID to database."""
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if doc:
            doc.mineru_task_id = task_id
            db.commit()
    finally:
        db.close()


def _clear_mineru_task_id(doc_id: int):
    """Clear MinerU task ID from database."""
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if doc:
            doc.mineru_task_id = None
            db.commit()
    finally:
        db.close()


def _save_parse_result(doc_id: int, md_path: str):
    """Save parse result to database."""
    from app.utils import to_relative_path
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if doc:
            # 存储相对路径
            doc.markdown_path = to_relative_path(md_path)
            doc.status = DocStatus.markdown_done
            doc.status_message = "PDF 解析完成"
            doc.progress = 100
            doc.mineru_task_id = None  # Clear task_id after success
            db.commit()
    finally:
        db.close()


async def _run_extract(doc_id: int):
    """Background task for extracting metadata."""
    await _with_semaphore(_do_extract_impl(doc_id))


async def _do_extract_impl(doc_id: int):
    """Extract implementation."""
    import traceback
    try:
        _update_doc_status(doc_id, DocStatus.extracting, "正在提取元数据...", 10)

        db = SessionLocal()
        try:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if not doc or not doc.markdown_path:
                _update_doc_status(doc_id, DocStatus.error, "Markdown 文件不存在")
                return

            # 已有标题、作者、年份则跳过提取
            if doc.title and doc.authors and doc.year:
                doc.status = DocStatus.meta_done
                doc.status_message = "元数据已存在，跳过提取"
                doc.progress = 100
                db.commit()
                return

            # 将相对路径转换为绝对路径
            abs_markdown_path = to_absolute_path(doc.markdown_path)
            with open(abs_markdown_path, "r", encoding="utf-8") as f:
                markdown_content = f.read()

            _update_doc_status(doc_id, DocStatus.extracting, "正在调用 Ollama...", 30)

            metadata = await extract_metadata(markdown_content)

            # 检查元数据是否有效
            title = metadata.get("title", "")
            authors = metadata.get("authors", [])
            doc_type = _normalize_doc_type(metadata.get("type", ""))

            if not title and not authors:
                _update_doc_status(doc_id, DocStatus.error, "元数据提取失败: 未能获取到标题和作者信息")
                return

            _update_doc_status(doc_id, DocStatus.extracting, "正在保存元数据...", 80)

            # 仅填充空字段
            if not doc.title:
                doc.title = title
            if not doc.authors:
                doc.authors = json.dumps(authors, ensure_ascii=False)
            if not doc.year:
                doc.year = metadata.get("year")
            if not doc.doi:
                doc.doi = metadata.get("doi", "")
            if not doc.source:
                doc.source = metadata.get("source", "")
            if not doc.journal:
                doc.journal = metadata.get("journal", "")
            if not doc.keywords:
                doc.keywords = json.dumps(metadata.get("keywords", []), ensure_ascii=False)
            if not doc.abstract:
                doc.abstract = metadata.get("abstract", "")
            if not doc.category:
                doc.category = metadata.get("category", "")
            if not doc.doc_type:
                doc.doc_type = doc_type
            if not doc.language:
                doc.language = metadata.get("language", "en")

            # English fields for non-English documents
            if doc.language and doc.language != "en":
                if not doc.title_en:
                    doc.title_en = metadata.get("title_en", "")
                if not doc.authors_en:
                    doc.authors_en = json.dumps(metadata.get("authors_en", []), ensure_ascii=False)
                if not doc.keywords_en:
                    doc.keywords_en = json.dumps(metadata.get("keywords_en", []), ensure_ascii=False)
                if not doc.abstract_en:
                    doc.abstract_en = metadata.get("abstract_en", "")
                if not doc.journal_en:
                    doc.journal_en = metadata.get("journal_en", "")

            doc.status = DocStatus.meta_done
            doc.status_message = "元数据提取完成"
            doc.progress = 100
            db.commit()
        finally:
            db.close()
    except Exception as e:
        error_msg = f"元数据提取失败: {type(e).__name__}: {str(e) or repr(e)}"
        print(f"[ERROR] Document {doc_id}: {error_msg}")
        print(traceback.format_exc())
        _update_doc_status(doc_id, DocStatus.error, error_msg)


async def _run_index(doc_id: int, vector_db_id: str = "default"):
    """Background task for indexing to specified vector database."""
    await _with_semaphore(_do_index_impl(doc_id, vector_db_id))


async def _do_index_impl(doc_id: int, vector_db_id: str = "default"):
    """Index implementation for a specific vector database."""
    db = SessionLocal()
    try:
        # 获取或创建索引状态记录
        index_record = db.query(DocumentVectorIndex).filter(
            DocumentVectorIndex.document_id == doc_id,
            DocumentVectorIndex.vector_db_id == vector_db_id
        ).first()

        if not index_record:
            index_record = DocumentVectorIndex(
                document_id=doc_id,
                vector_db_id=vector_db_id,
                status=IndexStatus.pending
            )
            db.add(index_record)
            db.commit()

        # 更新索引状态为 indexing
        index_record.status = IndexStatus.indexing
        index_record.error_message = None
        db.commit()

        _update_doc_status(doc_id, DocStatus.indexing, f"正在索引到 {vector_db_id}...", 10)

        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc or not doc.markdown_path:
            index_record.status = IndexStatus.error
            index_record.error_message = "Markdown 文件不存在"
            db.commit()
            _update_doc_status(doc_id, DocStatus.error, "Markdown 文件不存在")
            return

        # 获取向量数据库配置
        vdb_config = get_vector_db_by_id(vector_db_id)
        if not vdb_config:
            index_record.status = IndexStatus.error
            index_record.error_message = f"向量数据库 {vector_db_id} 配置不存在"
            db.commit()
            _update_doc_status(doc_id, DocStatus.error, f"向量数据库 {vector_db_id} 配置不存在")
            return

        # 将相对路径转换为绝对路径
        abs_markdown_path = to_absolute_path(doc.markdown_path)
        with open(abs_markdown_path, "r", encoding="utf-8") as f:
            markdown_content = f.read()

        _update_doc_status(doc_id, DocStatus.indexing, "正在生成向量...", 30)

        metadata = {
            "title": doc.title or "",
            "authors": json.loads(doc.authors) if doc.authors else [],
            "year": doc.year,
            "type": doc.doc_type or "",
            "keywords": json.loads(doc.keywords) if doc.keywords else [],
        }

        _update_doc_status(doc_id, DocStatus.indexing, "正在上传到向量数据库...", 60)

        # 创建绑定 vector_db_id 的 embedding 函数
        async def embedding_func(text):
            return await get_embedding(text, vector_db_id=vector_db_id)

        collection_name = await index_document(
            doc_id=doc.id,
            user_id=QDRANT_USER_ID,
            markdown_content=markdown_content,
            metadata=metadata,
            get_embedding_func=embedding_func,
            vector_db_id=vector_db_id,
        )

        # 更新索引记录
        index_record.collection_name = collection_name
        index_record.status = IndexStatus.indexed
        index_record.updated_at = datetime.now(timezone.utc)

        # 兼容：更新文档的旧字段（指向最新的索引）
        doc.qdrant_collection = collection_name
        doc.vector_db_id = vector_db_id
        doc.status = DocStatus.indexed
        doc.status_message = f"已索引到 {vector_db_id}"
        doc.progress = 100

        db.commit()
    except Exception as e:
        # 更新索引记录为错误状态
        if index_record:
            index_record.status = IndexStatus.error
            index_record.error_message = str(e)
            db.commit()
        _update_doc_status(doc_id, DocStatus.error, f"索引失败: {str(e)}")
    finally:
        db.close()


async def _run_full_pipeline(doc_id: int):
    """Background task for full pipeline."""
    await _with_semaphore(_do_full_pipeline_impl(doc_id))


async def _do_full_pipeline_impl(doc_id: int):
    """Full pipeline implementation."""
    try:
        db = SessionLocal()
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc:
            db.close()
            return

        file_path = doc.file_path
        db.close()

        # Stage 1: Parse
        _update_doc_status(doc_id, DocStatus.parsing, "阶段 1/3: 正在解析 PDF...", 0)
        await _do_parse_impl(doc_id, file_path)

        # Check if parse succeeded
        db = SessionLocal()
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc or doc.status != DocStatus.markdown_done:
            db.close()
            return
        db.close()

        # Stage 2: Extract
        _update_doc_status(doc_id, DocStatus.extracting, "阶段 2/3: 正在提取元数据...", 33)
        await _do_extract_impl(doc_id)

        # Check if extract succeeded
        db = SessionLocal()
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if not doc or doc.status != DocStatus.meta_done:
            db.close()
            return
        db.close()

        # Stage 3: Index
        _update_doc_status(doc_id, DocStatus.indexing, "阶段 3/3: 正在索引到 Qdrant...", 66)
        await _do_index_impl(doc_id)

    except Exception as e:
        _update_doc_status(doc_id, DocStatus.error, f"处理失败: {str(e)}")


@router.get("/queue/status")
def get_queue_status():
    """Get current task queue status."""
    return {
        "max_concurrent_tasks": MAX_CONCURRENT_TASKS,
        "running_tasks": _running_tasks,
        "available_slots": max(0, MAX_CONCURRENT_TASKS - _running_tasks),
    }


@router.get("/tasks/active")
def get_active_tasks(db: Session = Depends(get_db)):
    """Get all currently active background tasks."""
    tasks = []
    for doc_id, task_info in _active_tasks.items():
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if doc:
            tasks.append({
                "doc_id": doc_id,
                "doc_title": doc.title or doc.filename or f"文档 {doc_id}",
                "task_type": task_info["task_type"],
                "started_at": task_info["started_at"],
                "status_message": doc.status_message or task_info["status_message"],
                "status": doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
            })
    return {"tasks": tasks, "count": len(tasks)}


async def _process_batch():
    """Process all pending documents in batch mode with concurrency."""
    global _batch_paused

    # 一次性查询所有待处理文档
    db = SessionLocal()
    try:
        docs = db.query(Document).filter(
            Document.status.in_([DocStatus.uploaded, DocStatus.error])
        ).all()
        doc_ids = [doc.id for doc in docs]
    finally:
        db.close()

    if not doc_ids:
        _batch_paused = False
        return

    print(f"[Batch full] 开始全流程处理 {len(doc_ids)} 个文档，并发数: {MAX_CONCURRENT_TASKS}")

    # 为每个文档创建任务，Semaphore 会自动限制并发
    async def process_one(doc_id):
        if _batch_paused:
            return
        try:
            await _run_with_semaphore(doc_id)
        except Exception as e:
            print(f"[Batch full] 文档 {doc_id} 处理失败: {e}")

    tasks = [process_one(doc_id) for doc_id in doc_ids]
    await asyncio.gather(*tasks, return_exceptions=True)

    _batch_paused = False
    print(f"[Batch full] 全流程批量处理完成")


async def _run_with_semaphore(doc_id: int):
    """Run a single document pipeline with semaphore limiting."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        _track_task_start(doc_id, "full_pipeline")
        try:
            await _do_full_pipeline_impl(doc_id)
        except Exception as e:
            print(f"Pipeline error for doc {doc_id}: {e}")
        finally:
            _track_task_end(doc_id)
            _running_tasks -= 1


@router.post("/batch/start")
async def start_batch_processing(
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Start batch processing all pending documents."""
    global _batch_paused

    # Count pending documents
    pending_count = db.query(Document).filter(
        Document.status.in_([DocStatus.uploaded, DocStatus.error])
    ).count()

    if pending_count == 0:
        return {"detail": "没有待处理的文档", "pending": 0}

    _batch_paused = False
    background_tasks.add_task(_process_batch)

    return {
        "detail": f"批量处理已开始，共 {pending_count} 个文档待处理",
        "pending": pending_count,
    }


@router.post("/batch/pause")
def pause_batch_processing():
    """Pause batch processing after current tasks complete."""
    global _batch_paused
    _batch_paused = True
    return {"detail": "批量处理将在当前任务完成后暂停"}


@router.post("/batch/resume")
async def resume_batch_processing(
    background_tasks: BackgroundTasks,
):
    """Resume batch processing."""
    global _batch_paused
    _batch_paused = False
    background_tasks.add_task(_process_batch)
    return {"detail": "批量处理已恢复"}


async def _process_stage_batch(stage: str, filter_statuses: list[DocStatus], task_func):
    """Generic stage-specific batch processor with concurrency control."""
    global _batch_paused

    # 一次性查询所有待处理文档
    db = SessionLocal()
    try:
        docs = db.query(Document).filter(
            Document.status.in_(filter_statuses)
        ).all()
        doc_list = [(doc.id, doc.file_path) for doc in docs]
    finally:
        db.close()

    if not doc_list:
        _batch_paused = False
        return

    print(f"[Batch {stage}] 开始处理 {len(doc_list)} 个文档，并发数: {MAX_CONCURRENT_TASKS}")

    # 为每个文档创建任务，Semaphore 会自动限制并发
    async def process_one(doc_id, file_path):
        if _batch_paused:
            return
        try:
            await task_func(doc_id, file_path)
        except Exception as e:
            print(f"[Batch {stage}] 文档 {doc_id} 处理失败: {e}")

    tasks = [process_one(doc_id, file_path) for doc_id, file_path in doc_list]
    await asyncio.gather(*tasks, return_exceptions=True)

    _batch_paused = False
    print(f"[Batch {stage}] 批量处理完成")


async def _run_parse_only(doc_id: int, file_path: str = None):
    """Run only the parse stage."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        _track_task_start(doc_id, "parse")
        try:
            if not file_path:
                db = SessionLocal()
                try:
                    doc = db.query(Document).filter(Document.id == doc_id).first()
                    if doc:
                        file_path = doc.file_path
                finally:
                    db.close()
            await _do_parse_impl(doc_id, file_path)
        finally:
            _track_task_end(doc_id)
            _running_tasks -= 1


async def _run_extract_only(doc_id: int, _: str = None):
    """Run only the extract stage."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        _track_task_start(doc_id, "extract")
        try:
            await _do_extract_impl(doc_id)
        finally:
            _track_task_end(doc_id)
            _running_tasks -= 1


async def _run_index_only(doc_id: int, vector_db_id: str = "default"):
    """Run only the index stage for a specific vector database."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        _track_task_start(doc_id, "index")
        try:
            await _do_index_impl(doc_id, vector_db_id)
        finally:
            _track_task_end(doc_id)
            _running_tasks -= 1


async def _process_stage_batch_for_db(
    stage: str,
    filter_statuses: list[DocStatus],
    vector_db_id: str,
    limit: int = 0,
    batch_size: int = 0,
    pause_seconds: int = 0,
):
    """Batch processor for indexing to a specific vector database.

    Args:
        limit: 最多处理多少个文档，0 表示不限制
        batch_size: 每批处理多少个文档，0 表示不分批（全部同时处理）
        pause_seconds: 每批之间暂停多少秒
    """
    global _batch_paused, _batch_progress

    # 查询待索引文档
    db = SessionLocal()
    try:
        from app.models import DocumentVectorIndex, IndexStatus

        # 子查询：已索引到该数据库的文档 ID
        indexed_subq = db.query(DocumentVectorIndex.document_id).filter(
            DocumentVectorIndex.vector_db_id == vector_db_id,
            DocumentVectorIndex.status == IndexStatus.indexed,
        ).subquery()

        query = db.query(Document).filter(
            Document.status.in_(filter_statuses),
            ~Document.id.in_(indexed_subq),
        )

        # 按 ID 排序，确保顺序稳定
        query = query.order_by(Document.id)

        if limit > 0:
            query = query.limit(limit)

        docs = query.all()
        doc_ids = [doc.id for doc in docs]
    finally:
        db.close()

    if not doc_ids:
        _batch_paused = False
        _batch_progress["active"] = False
        return

    total = len(doc_ids)
    effective_batch_size = batch_size if batch_size > 0 else total
    num_batches = (total + effective_batch_size - 1) // effective_batch_size

    # 初始化进度
    _batch_progress.update({
        "active": True,
        "stage": stage,
        "vector_db_id": vector_db_id,
        "total": total,
        "processed": 0,
        "current_batch": 0,
        "total_batches": num_batches,
        "batch_size": batch_size,
        "pause_seconds": pause_seconds,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "errors": 0,
    })

    error_count = 0

    if batch_size <= 0 or batch_size >= total:
        # 不分批，一次性处理
        print(f"[Batch {stage}] 开始索引 {total} 个文档到 {vector_db_id}，并发数: {MAX_CONCURRENT_TASKS}")
        _batch_progress["current_batch"] = 1
        _batch_progress["total_batches"] = 1

        async def process_one(doc_id):
            nonlocal error_count
            if _batch_paused:
                return
            try:
                await _run_index_only(doc_id, vector_db_id)
            except Exception as e:
                error_count += 1
                print(f"[Batch {stage}] 文档 {doc_id} 索引失败: {e}")

        tasks = [process_one(doc_id) for doc_id in doc_ids]
        await asyncio.gather(*tasks, return_exceptions=True)
        _batch_progress["processed"] = total
        _batch_progress["errors"] = error_count
    else:
        # 分批处理
        batches = [doc_ids[i:i + batch_size] for i in range(0, total, batch_size)]
        print(f"[Batch {stage}] 开始分批索引 {total} 个文档到 {vector_db_id}，"
              f"每批 {batch_size} 个，暂停 {pause_seconds}s，共 {len(batches)} 批")

        processed = 0
        for batch_idx, batch_ids in enumerate(batches):
            if _batch_paused:
                print(f"[Batch {stage}] 已暂停，中止剩余批次")
                break

            _batch_progress["current_batch"] = batch_idx + 1

            print(f"[Batch {stage}] 处理第 {batch_idx + 1}/{len(batches)} 批，"
                  f"文档 {batch_ids[0]}-{batch_ids[-1]}")

            async def process_one(doc_id):
                nonlocal error_count
                if _batch_paused:
                    return
                try:
                    await _run_index_only(doc_id, vector_db_id)
                except Exception as e:
                    error_count += 1
                    print(f"[Batch {stage}] 文档 {doc_id} 索引失败: {e}")

            tasks = [process_one(doc_id) for doc_id in batch_ids]
            await asyncio.gather(*tasks, return_exceptions=True)

            processed += len(batch_ids)
            _batch_progress["processed"] = processed
            _batch_progress["errors"] = error_count

            # 批间暂停（最后一批不暂停）
            if pause_seconds > 0 and batch_idx < len(batches) - 1 and not _batch_paused:
                print(f"[Batch {stage}] 批间暂停 {pause_seconds}s...")
                await asyncio.sleep(pause_seconds)

    _batch_paused = False
    _batch_progress["active"] = False
    print(f"[Batch {stage}] 批量索引完成，共处理 {total} 个文档，失败 {error_count} 个")


@router.post("/batch/start-parse")
async def start_batch_parse(background_tasks: BackgroundTasks, db: Session = Depends(get_db)):
    """Start batch parsing for all uploaded documents."""
    global _batch_paused
    count = db.query(Document).filter(Document.status.in_([DocStatus.uploaded, DocStatus.error])).count()
    if count == 0:
        return {"detail": "没有待解析的文档", "pending": 0}
    _batch_paused = False
    background_tasks.add_task(_process_stage_batch, "parse", [DocStatus.uploaded, DocStatus.error], _run_parse_only)
    return {"detail": f"批量解析已开始，共 {count} 个文档", "pending": count}


@router.post("/batch/start-extract")
async def start_batch_extract(background_tasks: BackgroundTasks, db: Session = Depends(get_db)):
    """Start batch metadata extraction for all parsed documents."""
    global _batch_paused
    count = db.query(Document).filter(Document.status == DocStatus.markdown_done).count()
    if count == 0:
        return {"detail": "没有待提取元数据的文档", "pending": 0}
    _batch_paused = False
    background_tasks.add_task(_process_stage_batch, "extract", [DocStatus.markdown_done], _run_extract_only)
    return {"detail": f"批量提取已开始，共 {count} 个文档", "pending": count}


@router.post("/batch/start-index")
async def start_batch_index(
    vector_db_id: str = "default",
    limit: int = 0,
    batch_size: int = 0,
    pause_seconds: int = 0,
    background_tasks: BackgroundTasks = BackgroundTasks(),
    db: Session = Depends(get_db),
):
    """Start batch indexing for all metadata-extracted documents to specified vector database.

    Args:
        vector_db_id: 向量数据库 ID
        limit: 最多索引多少个文档，0 表示不限制
        batch_size: 每批处理多少个文档，0 表示不分批
        pause_seconds: 每批之间暂停多少秒
    """
    global _batch_paused

    # 验证向量数据库配置
    vdb_config = get_vector_db_by_id(vector_db_id)
    if not vdb_config:
        raise HTTPException(status_code=400, detail=f"向量数据库 {vector_db_id} 配置不存在")

    # 查找可索引的文档（meta_done 或已有索引但需要索引到新数据库的）
    from app.models import DocumentVectorIndex, IndexStatus as IndexStatusEnum
    indexed_subq = db.query(DocumentVectorIndex.document_id).filter(
        DocumentVectorIndex.vector_db_id == vector_db_id,
        DocumentVectorIndex.status == IndexStatusEnum.indexed,
    ).subquery()

    count_query = db.query(Document).filter(
        Document.status.in_([DocStatus.meta_done, DocStatus.indexed]),
        ~Document.id.in_(indexed_subq),
    )

    total_available = count_query.count()

    if total_available == 0:
        return {"detail": "没有待索引的文档", "pending": 0}

    # 实际处理数量
    actual_limit = min(limit, total_available) if limit > 0 else total_available

    _batch_paused = False
    background_tasks.add_task(
        _process_stage_batch_for_db,
        "index",
        [DocStatus.meta_done, DocStatus.indexed],
        vector_db_id,
        limit,
        batch_size,
        pause_seconds,
    )

    # 构建描述信息
    detail_parts = [f"批量索引到 {vector_db_id} 已开始，共 {actual_limit} 个文档"]
    if batch_size > 0 and batch_size < actual_limit:
        detail_parts.append(f"每批 {batch_size} 个")
    if pause_seconds > 0:
        detail_parts.append(f"批间暂停 {pause_seconds}s")

    return {
        "detail": "，".join(detail_parts),
        "pending": actual_limit,
        "total_available": total_available,
    }


@router.post("/batch/start-full")
async def start_batch_full(background_tasks: BackgroundTasks, db: Session = Depends(get_db)):
    """Start full pipeline batch for all pending documents."""
    global _batch_paused
    count = db.query(Document).filter(
        Document.status.in_([DocStatus.uploaded, DocStatus.error])
    ).count()
    if count == 0:
        return {"detail": "没有待处理的文档", "pending": 0}
    _batch_paused = False
    background_tasks.add_task(_process_batch)
    return {"detail": f"全流程批量处理已开始，共 {count} 个文档", "pending": count}


@router.get("/batch/status")
def get_batch_status(db: Session = Depends(get_db)):
    """Get batch processing status."""
    pending_count = db.query(Document).filter(
        Document.status.in_([DocStatus.uploaded, DocStatus.error])
    ).count()

    processing_count = db.query(Document).filter(
        Document.status.in_([DocStatus.parsing, DocStatus.extracting, DocStatus.indexing])
    ).count()

    completed_count = db.query(Document).filter(
        Document.status == DocStatus.indexed
    ).count()

    total_count = db.query(Document).count()

    # 统计超时失败的任务数（status_message 包含 "timed out"）
    timeout_error_count = db.query(Document).filter(
        Document.status == DocStatus.error,
        Document.status_message.contains("timed out")
    ).count()

    # 统计已解析且已有标题和作者的文档数（可直接标记为已提取）
    ready_to_promote = db.query(Document).filter(
        Document.status == DocStatus.markdown_done,
        Document.title.isnot(None), Document.title != "",
        Document.authors.isnot(None), Document.authors != "",
    ).count()

    return {
        "paused": _batch_paused,
        "pending": pending_count,
        "processing": processing_count,
        "completed": completed_count,
        "total": total_count,
        "running_tasks": _running_tasks,
        "max_concurrent": MAX_CONCURRENT_TASKS,
        "stage_counts": {
            "uploaded": db.query(Document).filter(Document.status == DocStatus.uploaded).count(),
            "error": db.query(Document).filter(Document.status == DocStatus.error).count(),
            "timeout_error": timeout_error_count,
            "markdown_done": db.query(Document).filter(Document.status == DocStatus.markdown_done).count(),
            "meta_done": db.query(Document).filter(Document.status == DocStatus.meta_done).count(),
            "ready_to_promote": ready_to_promote,
        },
        "batch_progress": dict(_batch_progress),
    }


@router.post("/batch/reset-errors")
def batch_reset_errors(target_status: str = None, db: Session = Depends(get_db)):
    """批量将所有错误状态的文档重置为失败前的状态。

    默认重置逻辑：
    - 有 markdown_path 的文档 -> markdown_done
    - 没有 markdown_path 的文档 -> uploaded
    """
    # 查询所有错误状态的文档
    error_docs = db.query(Document).filter(Document.status == DocStatus.error).all()

    if not error_docs:
        return {"detail": "没有错误状态的文档", "reset_count": 0}

    reset_count = 0
    for doc in error_docs:
        # 根据目标状态或默认逻辑确定重置状态
        if target_status and target_status in ['uploaded', 'markdown_done', 'meta_done']:
            doc.status = DocStatus(target_status)
        else:
            # 默认逻辑：根据是否有 markdown 文件决定
            if doc.markdown_path:
                doc.status = DocStatus.markdown_done
            else:
                doc.status = DocStatus.uploaded

        doc.status_message = None
        doc.progress = 0
        doc.mineru_task_id = None
        reset_count += 1

    db.commit()

    return {
        "detail": f"已重置 {reset_count} 个错误状态的文档",
        "reset_count": reset_count,
    }


@router.post("/batch/reset-timeout-errors")
def batch_reset_timeout_errors(target_status: str = None, db: Session = Depends(get_db)):
    """批量将超时失败的文档重置为失败前的状态。

    仅重置 status_message 包含 "timed out" 的错误文档。
    默认重置逻辑：
    - 有 markdown_path 的文档 -> markdown_done
    - 没有 markdown_path 的文档 -> uploaded
    """
    # 查询超时失败的文档
    timeout_docs = db.query(Document).filter(
        Document.status == DocStatus.error,
        Document.status_message.contains("timed out")
    ).all()

    if not timeout_docs:
        return {"detail": "没有超时失败的文档", "reset_count": 0}

    reset_count = 0
    for doc in timeout_docs:
        if target_status and target_status in ['uploaded', 'markdown_done', 'meta_done']:
            doc.status = DocStatus(target_status)
        else:
            if doc.markdown_path:
                doc.status = DocStatus.markdown_done
            else:
                doc.status = DocStatus.uploaded

        doc.status_message = None
        doc.progress = 0
        doc.mineru_task_id = None
        reset_count += 1

    db.commit()

    return {
        "detail": f"已重置 {reset_count} 个超时失败的文档",
        "reset_count": reset_count,
    }


@router.post("/batch/promote-ready")
def batch_promote_ready(db: Session = Depends(get_db)):
    """将已解析且包含标题和作者信息的文档直接标记为已提取。

    跳过 Ollama 元数据提取步骤，适用于已通过其他方式（如 Zotero 导入）
    填充了元数据的文档。
    """
    ready_docs = db.query(Document).filter(
        Document.status == DocStatus.markdown_done,
        Document.title.isnot(None), Document.title != "",
        Document.authors.isnot(None), Document.authors != "",
    ).all()

    if not ready_docs:
        return {"detail": "没有符合条件的文档", "promote_count": 0}

    promote_count = 0
    for doc in ready_docs:
        doc.status = DocStatus.meta_done
        doc.status_message = "元数据已存在，跳过提取"
        doc.progress = 100
        promote_count += 1

    db.commit()

    return {
        "detail": f"已将 {promote_count} 个文档标记为已提取",
        "promote_count": promote_count,
    }


@router.post("/{doc_id}/reset")
def reset_document(
    doc_id: int,
    target_status: str = None,
    db: Session = Depends(get_db),
):
    """Reset document status to a previous state for re-processing."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    # Determine target status based on current status
    # Allow resetting to any previous valid state
    # markdown_done can be reset to meta_done (user can fill metadata manually)
    reset_map = {
        DocStatus.parsing: [DocStatus.uploaded],
        DocStatus.markdown_done: [DocStatus.uploaded, DocStatus.meta_done],
        DocStatus.extracting: [DocStatus.uploaded, DocStatus.markdown_done],
        DocStatus.meta_done: [DocStatus.uploaded, DocStatus.markdown_done],
        DocStatus.indexing: [DocStatus.uploaded, DocStatus.markdown_done, DocStatus.meta_done],
        DocStatus.indexed: [DocStatus.uploaded, DocStatus.markdown_done, DocStatus.meta_done],
        DocStatus.error: [DocStatus.uploaded, DocStatus.markdown_done, DocStatus.meta_done],
    }

    valid_targets = reset_map.get(doc.status, [DocStatus.uploaded])

    # If target_status is provided, use it; otherwise default to first valid target
    if target_status and target_status in [s.value for s in valid_targets]:
        doc.status = DocStatus(target_status)
    else:
        doc.status = valid_targets[0]

    doc.status_message = None
    doc.progress = 0
    doc.mineru_task_id = None  # Clear MinerU task tracking on reset

    # Clear downstream data based on target status
    if doc.status == DocStatus.uploaded:
        doc.markdown_path = None
        doc.qdrant_collection = None
    elif doc.status == DocStatus.markdown_done:
        doc.qdrant_collection = None

    db.commit()

    return {
        "detail": f"状态已重置为 {doc.status.value}",
        "status": doc.status.value,
        "valid_targets": [s.value for s in valid_targets],
    }


@router.post("/{doc_id}/parse")
async def parse_document(
    doc_id: int,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Stage 1: Parse PDF to Markdown using MinerU (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    # Allow parsing from uploaded or error status (for retry after timeout)
    if doc.status not in [DocStatus.uploaded, DocStatus.error]:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许解析")

    doc.status = DocStatus.parsing
    doc.status_message = "任务已提交，等待处理..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_parse, doc_id, doc.file_path)

    return {"detail": "解析任务已提交", "status": "parsing"}


@router.post("/{doc_id}/extract")
async def extract_document_meta(
    doc_id: int,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Stage 2: Extract metadata using Ollama (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.markdown_done:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许提取元数据")

    doc.status = DocStatus.extracting
    doc.status_message = "任务已提交，等待处理..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_extract, doc_id)

    return {"detail": "元数据提取任务已提交", "status": "extracting"}


@router.post("/{doc_id}/index")
async def index_document_to_qdrant(
    doc_id: int,
    vector_db_id: str = "default",
    background_tasks: BackgroundTasks = BackgroundTasks(),
    db: Session = Depends(get_db),
):
    """Stage 3: Index document into specified vector database (async).

    Args:
        doc_id: 文档 ID
        vector_db_id: 向量数据库 ID，默认为 "default"
    """
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    # 允许从以下状态进行索引：
    # - meta_done: 元数据已提取
    # - indexed: 已索引到其他数据库，支持索引到更多数据库
    # - markdown_done: 有 markdown 文件即可索引
    # - error: 重试索引
    allowed_statuses = [DocStatus.meta_done, DocStatus.indexed, DocStatus.error, DocStatus.markdown_done]
    if doc.status not in allowed_statuses:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许索引")

    # 检查是否有 markdown 文件
    if not doc.markdown_path:
        raise HTTPException(status_code=400, detail="Markdown 文件不存在，无法索引")

    # 验证向量数据库配置是否存在
    vdb_config = get_vector_db_by_id(vector_db_id)
    if not vdb_config:
        raise HTTPException(status_code=400, detail=f"向量数据库 {vector_db_id} 配置不存在")

    doc.status = DocStatus.indexing
    doc.status_message = f"正在索引到 {vector_db_id}..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_index, doc_id, vector_db_id)

    return {"detail": f"索引任务已提交到 {vector_db_id}", "status": "indexing"}


@router.get("/{doc_id}/indexes")
async def get_document_indexes(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """获取文档在各向量数据库中的索引状态。"""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    indexes = db.query(DocumentVectorIndex).filter(
        DocumentVectorIndex.document_id == doc_id
    ).all()

    return {
        "document_id": doc_id,
        "indexes": [
            {
                "vector_db_id": idx.vector_db_id,
                "collection_name": idx.collection_name,
                "status": idx.status.value if isinstance(idx.status, IndexStatus) else idx.status,
                "error_message": idx.error_message,
                "updated_at": idx.updated_at.isoformat() if idx.updated_at else None,
            }
            for idx in indexes
        ]
    }


@router.post("/{doc_id}/process")
async def process_full_pipeline(
    doc_id: int,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Run all three stages in sequence (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status not in [DocStatus.uploaded, DocStatus.error]:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许处理")

    doc.status = DocStatus.parsing
    doc.status_message = "全流程处理已提交..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_full_pipeline, doc_id)

    return {"detail": "全流程处理任务已提交", "status": "processing"}


@router.get("/{doc_id}/status")
def get_pipeline_status(
    doc_id: int,
    db: Session = Depends(get_db),
):
    """Get current processing status of a document."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    # 检查 markdown 文件是否存在
    has_markdown = False
    if doc.markdown_path:
        abs_markdown_path = to_absolute_path(doc.markdown_path)
        has_markdown = os.path.exists(abs_markdown_path)

    # 获取索引状态
    indexes = db.query(DocumentVectorIndex).filter(
        DocumentVectorIndex.document_id == doc_id
    ).all()

    indexed_dbs = [
        {
            "vector_db_id": idx.vector_db_id,
            "status": idx.status.value if isinstance(idx.status, IndexStatus) else idx.status,
        }
        for idx in indexes
        if idx.status == IndexStatus.indexed
    ]

    return {
        "status": doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
        "status_message": doc.status_message,
        "progress": doc.progress,
        "has_markdown": has_markdown,
        "has_meta": doc.title is not None,
        "is_indexed": len(indexed_dbs) > 0,
        "indexed_dbs": indexed_dbs,
    }
