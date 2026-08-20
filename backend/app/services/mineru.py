import os
import asyncio
import zipfile
from pathlib import Path
from io import BytesIO

import httpx
from app.config import get_settings
from app.mineru_fast_api.client import AuthenticatedClient
from app.mineru_fast_api.api.default import (
    submit_parse_task_tasks_post,
    get_router_task_status_tasks_task_id_get,
)
from app.mineru_fast_api.models.body_submit_parse_task_tasks_post import BodySubmitParseTaskTasksPost
from app.mineru_fast_api.types import File

settings = get_settings()


def _get_client() -> AuthenticatedClient:
    """Create a new MinerU client instance."""
    return AuthenticatedClient(
        base_url=settings.mineru_url,
        token=settings.mineru_key,
        prefix="Bearer",
        timeout=300.0,
    )


async def check_task_status(task_id: str) -> dict:
    """
    Check the status of an existing MinerU task.
    Returns a dict with 'status' key: 'completed', 'failed', 'processing', or 'unknown'.
    """
    try:
        async with _get_client() as client:
            status_result = await get_router_task_status_tasks_task_id_get.asyncio(
                task_id=task_id, client=client
            )
        if not status_result or not isinstance(status_result, dict):
            return {"status": "unknown"}
        return status_result
    except Exception:
        return {"status": "unknown"}


async def get_task_result(task_id: str, output_dir: str, doc_id: int = None) -> str:
    """
    Fetch the result of a completed MinerU task and save to output_dir.
    Images are packed into images.zip instead of a directory.
    Returns the path to the generated markdown file.
    """
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)

    headers = {"Authorization": f"Bearer {settings.mineru_key}"}
    async with httpx.AsyncClient(timeout=300.0) as http_client:
        response = await http_client.get(
            f"{settings.mineru_url}/tasks/{task_id}/result",
            headers=headers,
        )

    if response.status_code != 200:
        raise Exception(f"Failed to get MinerU result [task_id={task_id}]: HTTP {response.status_code}")

    # Process ZIP file
    zip_content = response.content
    markdown_content = ""
    image_entries = []  # (basename, data)

    try:
        with zipfile.ZipFile(BytesIO(zip_content), 'r') as zip_ref:
            for file_info in zip_ref.infolist():
                if file_info.filename.endswith('.md'):
                    # Read markdown content with error handling
                    with zip_ref.open(file_info) as md_file:
                        raw_data = md_file.read()
                        for encoding in ['utf-8', 'gbk', 'gb2312', 'latin-1']:
                            try:
                                markdown_content = raw_data.decode(encoding)
                                break
                            except (UnicodeDecodeError, ValueError):
                                continue
                        else:
                            markdown_content = raw_data.decode('utf-8', errors='replace')
                elif '/' in file_info.filename:
                    # Collect image files
                    filename = os.path.basename(file_info.filename)
                    if filename and any(filename.lower().endswith(ext) for ext in ['.png', '.jpg', '.jpeg', '.gif', '.svg']):
                        with zip_ref.open(file_info) as img_file:
                            image_entries.append((filename, img_file.read()))
    except zipfile.BadZipFile:
        # If not a ZIP, try to use as JSON response
        try:
            result = response.json()
            if isinstance(result, dict) and "results" in result:
                results = result["results"]
                for filename, data in results.items():
                    if isinstance(data, dict) and "md_content" in data:
                        markdown_content = data["md_content"]
                        break
        except Exception:
            raise Exception(f"Invalid response format from MinerU [task_id={task_id}]")

    if not markdown_content:
        raise Exception(f"No markdown content found in MinerU result [task_id={task_id}]")

    # Save images to images.zip
    images_zip_path = output_path / "images.zip"
    if image_entries:
        with zipfile.ZipFile(images_zip_path, 'w', zipfile.ZIP_STORED) as zf:
            for filename, data in image_entries:
                zf.writestr(filename, data)

    # Remove legacy images/ directory if it exists
    legacy_images_dir = output_path / "images"
    if legacy_images_dir.exists():
        import shutil
        shutil.rmtree(legacy_images_dir)

    # Save markdown as {id}.md
    md_filename = f"{doc_id}.md" if doc_id else "output.md"
    md_path = output_path / md_filename
    with open(md_path, "w", encoding="utf-8") as f:
        f.write(markdown_content)

    return str(md_path)


async def submit_parse_task(file_path: str) -> str:
    """
    Submit a PDF parsing task to MinerU and return the task_id.
    Does NOT wait for completion.
    """
    with open(file_path, "rb") as f:
        pdf_content = f.read()

    file_obj = File(
        payload=BytesIO(pdf_content),
        file_name=os.path.basename(file_path),
        mime_type="application/pdf",
    )

    body = BodySubmitParseTaskTasksPost(
        files=[file_obj],
        return_md=True,
        backend="pipeline",
        parse_method="auto",
        formula_enable=True,
        table_enable=True,
        image_analysis=False,
        response_format_zip=True,
        return_images=True,
    )

    async with _get_client() as client:
        task_result = await submit_parse_task_tasks_post.asyncio(client=client, body=body)

    if not task_result or not isinstance(task_result, dict):
        raise Exception(f"Failed to submit MinerU task: invalid response {task_result}")

    task_id = task_result.get("task_id")
    if not task_id:
        raise Exception(f"MinerU task submission failed, no task_id in response: {task_result}")

    return task_id


async def poll_task(task_id: str, max_wait: int = None, poll_interval: int = 2) -> dict:
    """
    Poll a MinerU task until completion, failure, or timeout.
    Returns the final status result dict.
    Raises Exception on failure or timeout.
    """
    if max_wait is None:
        max_wait = settings.mineru_task_timeout
    for _ in range(max_wait // poll_interval):
        async with _get_client() as client:
            status_result = await get_router_task_status_tasks_task_id_get.asyncio(
                task_id=task_id, client=client
            )

        if not status_result or not isinstance(status_result, dict):
            await asyncio.sleep(poll_interval)
            continue

        status = status_result.get("status")
        if status == "completed":
            return status_result
        elif status == "failed":
            error = status_result.get("error", "Unknown error")
            raise Exception(f"MinerU task failed [task_id={task_id}]: {error}")

        await asyncio.sleep(poll_interval)

    raise Exception(f"MinerU task timed out [task_id={task_id}, timeout={max_wait}s]")


async def parse_pdf(file_path: str, output_dir: str, doc_id: int = None) -> str:
    """
    Call MinerU API asynchronously to parse PDF into Markdown.
    Returns the path to the generated markdown file.
    """
    task_id = await submit_parse_task(file_path)
    await poll_task(task_id)
    return await get_task_result(task_id, output_dir)
