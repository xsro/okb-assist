import json
import os
import asyncio
from typing import Optional

from fastapi import APIRouter, Depends, HTTPException, BackgroundTasks
from sqlalchemy.orm import Session

from app.config import get_settings
from app.database import get_db, SessionLocal
from app.models import Document, DocStatus
from app.services.mineru import parse_pdf, submit_parse_task, poll_task, get_task_result, check_task_status
from app.services.ollama import extract_metadata, get_embedding, add_yaml_frontmatter
from app.services.qdrant import index_document, delete_document_points

router = APIRouter(prefix="/assist/api/pipeline", tags=["pipeline"])

QDRANT_USER_ID = 0  # Default user ID for Qdrant since no auth

# Concurrency limiting
settings = get_settings()
MAX_CONCURRENT_TASKS = getattr(settings, 'max_concurrent_tasks', 3)
_task_semaphore = asyncio.Semaphore(MAX_CONCURRENT_TASKS)
_running_tasks = 0
_batch_paused = False  # Flag to pause batch processing


def _get_running_tasks() -> int:
    """Get number of currently running tasks."""
    return _running_tasks


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

        # Save generated files in uploads/{doc_id}/
        output_dir = os.path.join("uploads", str(doc_id))
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
                md_path = await get_task_result(existing_task_id, output_dir)
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
                    md_path = await get_task_result(existing_task_id, output_dir)
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

        # Submit a new task
        task_id = await submit_parse_task(file_path)

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
        md_path = await get_task_result(task_id, output_dir)

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
    db = SessionLocal()
    try:
        doc = db.query(Document).filter(Document.id == doc_id).first()
        if doc:
            doc.markdown_path = md_path
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

            with open(doc.markdown_path, "r", encoding="utf-8") as f:
                markdown_content = f.read()

            _update_doc_status(doc_id, DocStatus.extracting, "正在调用 Ollama...", 30)

            metadata = await extract_metadata(markdown_content)

            # 检查元数据是否有效
            title = metadata.get("title", "")
            authors = metadata.get("authors", [])
            doc_type = metadata.get("type", "")

            if not title and not authors:
                # 元数据提取失败，返回空内容
                _update_doc_status(doc_id, DocStatus.error, "元数据提取失败: 未能获取到标题和作者信息")
                return

            _update_doc_status(doc_id, DocStatus.extracting, "正在保存元数据...", 80)

            # Update document fields
            doc.title = title
            doc.authors = json.dumps(authors, ensure_ascii=False)
            doc.year = metadata.get("year")
            doc.doi = metadata.get("doi", "")
            doc.source = metadata.get("source", "")
            doc.journal = metadata.get("journal", "")
            doc.keywords = json.dumps(metadata.get("keywords", []), ensure_ascii=False)
            doc.abstract = metadata.get("abstract", "")
            doc.category = metadata.get("category", "")
            doc.doc_type = doc_type
            doc.language = metadata.get("language", "en")

            # English fields for non-English documents
            if doc.language and doc.language != "en":
                doc.title_en = metadata.get("title_en", "")
                doc.authors_en = json.dumps(metadata.get("authors_en", []), ensure_ascii=False)
                doc.keywords_en = json.dumps(metadata.get("keywords_en", []), ensure_ascii=False)
                doc.abstract_en = metadata.get("abstract_en", "")
                doc.journal_en = metadata.get("journal_en", "")

            # Add YAML frontmatter to markdown
            updated_markdown = add_yaml_frontmatter(markdown_content, metadata)
            with open(doc.markdown_path, "w", encoding="utf-8") as f:
                f.write(updated_markdown)

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


async def _run_index(doc_id: int):
    """Background task for indexing to Qdrant."""
    await _with_semaphore(_do_index_impl(doc_id))


async def _do_index_impl(doc_id: int):
    """Index implementation."""
    try:
        _update_doc_status(doc_id, DocStatus.indexing, "正在索引到 Qdrant...", 10)

        db = SessionLocal()
        try:
            doc = db.query(Document).filter(Document.id == doc_id).first()
            if not doc or not doc.markdown_path:
                _update_doc_status(doc_id, DocStatus.error, "Markdown 文件不存在")
                return

            with open(doc.markdown_path, "r", encoding="utf-8") as f:
                markdown_content = f.read()

            _update_doc_status(doc_id, DocStatus.indexing, "正在生成向量...", 30)

            metadata = {
                "title": doc.title or "",
                "authors": json.loads(doc.authors) if doc.authors else [],
                "year": doc.year,
                "type": doc.doc_type or "",
                "keywords": json.loads(doc.keywords) if doc.keywords else [],
            }

            _update_doc_status(doc_id, DocStatus.indexing, "正在上传到 Qdrant...", 60)

            collection_name = await index_document(
                doc_id=doc.id,
                user_id=QDRANT_USER_ID,
                markdown_content=markdown_content,
                metadata=metadata,
                get_embedding_func=get_embedding,
            )

            doc.qdrant_collection = collection_name
            doc.status = DocStatus.indexed
            doc.status_message = "已索引到向量数据库"
            doc.progress = 100
            db.commit()
        finally:
            db.close()
    except Exception as e:
        _update_doc_status(doc_id, DocStatus.error, f"索引失败: {str(e)}")


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


async def _process_batch():
    """Process all pending documents in batch mode with concurrency."""
    global _batch_paused

    # Track active tasks
    active_tasks = set()

    while not _batch_paused:
        # Clean up completed tasks
        active_tasks = {t for t in active_tasks if not t.done()}

        # Check if we can start more tasks
        available_slots = MAX_CONCURRENT_TASKS - len(active_tasks)

        if available_slots > 0:
            db = SessionLocal()
            try:
                # Find pending documents
                docs = db.query(Document).filter(
                    Document.status.in_([DocStatus.uploaded, DocStatus.error])
                ).limit(available_slots).all()

                if not docs and not active_tasks:
                    # No more documents and no active tasks
                    break

                for doc in docs:
                    doc_id = doc.id
                    # Create task for each document
                    task = asyncio.create_task(_run_with_semaphore(doc_id))
                    active_tasks.add(task)

            except Exception as e:
                print(f"Batch processing error: {e}")
            finally:
                db.close()

        # Wait a bit before checking again
        await asyncio.sleep(2)

    # Wait for all active tasks to complete
    if active_tasks:
        await asyncio.gather(*active_tasks, return_exceptions=True)

    _batch_paused = False


async def _run_with_semaphore(doc_id: int):
    """Run a single document pipeline with semaphore limiting."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        try:
            await _do_full_pipeline_impl(doc_id)
        except Exception as e:
            print(f"Pipeline error for doc {doc_id}: {e}")
        finally:
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
    active_tasks = set()

    while not _batch_paused:
        active_tasks = {t for t in active_tasks if not t.done()}
        available_slots = MAX_CONCURRENT_TASKS - len(active_tasks)

        if available_slots > 0:
            db = SessionLocal()
            try:
                docs = db.query(Document).filter(
                    Document.status.in_(filter_statuses)
                ).limit(available_slots).all()

                if not docs and not active_tasks:
                    break

                for doc in docs:
                    task = asyncio.create_task(task_func(doc.id, doc.file_path if hasattr(doc, 'file_path') else None))
                    active_tasks.add(task)
            except Exception as e:
                print(f"Batch {stage} error: {e}")
            finally:
                db.close()

        await asyncio.sleep(2)

    if active_tasks:
        await asyncio.gather(*active_tasks, return_exceptions=True)
    _batch_paused = False


async def _run_parse_only(doc_id: int, file_path: str = None):
    """Run only the parse stage."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
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
            _running_tasks -= 1


async def _run_extract_only(doc_id: int, _: str = None):
    """Run only the extract stage."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        try:
            await _do_extract_impl(doc_id)
        finally:
            _running_tasks -= 1


async def _run_index_only(doc_id: int, _: str = None):
    """Run only the index stage."""
    global _running_tasks
    async with _task_semaphore:
        _running_tasks += 1
        try:
            await _do_index_impl(doc_id)
        finally:
            _running_tasks -= 1


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
async def start_batch_index(background_tasks: BackgroundTasks, db: Session = Depends(get_db)):
    """Start batch indexing for all metadata-extracted documents."""
    global _batch_paused
    count = db.query(Document).filter(Document.status == DocStatus.meta_done).count()
    if count == 0:
        return {"detail": "没有待索引的文档", "pending": 0}
    _batch_paused = False
    background_tasks.add_task(_process_stage_batch, "index", [DocStatus.meta_done], _run_index_only)
    return {"detail": f"批量索引已开始，共 {count} 个文档", "pending": count}


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
            "markdown_done": db.query(Document).filter(Document.status == DocStatus.markdown_done).count(),
            "meta_done": db.query(Document).filter(Document.status == DocStatus.meta_done).count(),
        },
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
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Stage 3: Index document into Qdrant (async)."""
    doc = db.query(Document).filter(Document.id == doc_id).first()
    if not doc:
        raise HTTPException(status_code=404, detail="文献不存在")

    if doc.status != DocStatus.meta_done:
        raise HTTPException(status_code=400, detail=f"当前状态 {doc.status.value} 不允许索引")

    doc.status = DocStatus.indexing
    doc.status_message = "任务已提交，等待处理..."
    doc.progress = 0
    db.commit()

    background_tasks.add_task(_run_index, doc_id)

    return {"detail": "索引任务已提交", "status": "indexing"}


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

    return {
        "status": doc.status.value if isinstance(doc.status, DocStatus) else doc.status,
        "status_message": doc.status_message,
        "progress": doc.progress,
        "has_markdown": doc.markdown_path is not None and os.path.exists(doc.markdown_path or ""),
        "has_meta": doc.title is not None,
        "is_indexed": doc.qdrant_collection is not None,
    }
