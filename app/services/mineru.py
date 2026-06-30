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


async def parse_pdf(file_path: str, output_dir: str) -> str:
    """
    Call MinerU API asynchronously to parse PDF into Markdown.
    Returns the path to the generated markdown file.
    """
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)

    # Read the PDF file
    with open(file_path, "rb") as f:
        pdf_content = f.read()

    # Create file object for upload
    file_obj = File(
        payload=BytesIO(pdf_content),
        file_name=os.path.basename(file_path),
        mime_type="application/pdf",
    )

    # Create request body for async task
    body = BodySubmitParseTaskTasksPost(
        files=[file_obj],
        return_md=True,
        backend="pipeline",
        parse_method="auto",
        formula_enable=True,
        table_enable=True,
        image_analysis=False,  # Disable image analysis
        response_format_zip=True,  # Return ZIP to include images
        return_images=True,  # Include extracted images
    )

    # Submit async task
    async with _get_client() as client:
        task_result = await submit_parse_task_tasks_post.asyncio(client=client, body=body)

    if not task_result or not isinstance(task_result, dict):
        raise Exception("Failed to submit MinerU task")

    task_id = task_result.get("task_id")
    if not task_id:
        raise Exception(f"MinerU task submission failed: {task_result}")

    # Poll for task completion
    max_wait = 300  # 5 minutes max
    poll_interval = 2  # 2 seconds between polls

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
            break
        elif status == "failed":
            error = status_result.get("error", "Unknown error")
            raise Exception(f"MinerU task failed: {error}")

        await asyncio.sleep(poll_interval)
    else:
        raise Exception("MinerU task timed out")

    # Get result as ZIP using httpx directly (the generated client tries to parse as JSON)
    headers = {"Authorization": f"Bearer {settings.mineru_key}"}
    async with httpx.AsyncClient(timeout=300.0) as http_client:
        response = await http_client.get(
            f"{settings.mineru_url}/tasks/{task_id}/result",
            headers=headers,
        )

    if response.status_code != 200:
        raise Exception(f"Failed to get MinerU result: {response.status_code}")

    # Process ZIP file
    zip_content = response.content
    markdown_content = ""
    images_dir = output_path / "images"
    images_dir.mkdir(exist_ok=True)

    try:
        with zipfile.ZipFile(BytesIO(zip_content), 'r') as zip_ref:
            # Extract all files
            for file_info in zip_ref.infolist():
                if file_info.filename.endswith('.md'):
                    # Read markdown content with error handling
                    with zip_ref.open(file_info) as md_file:
                        raw_data = md_file.read()
                        # Try different encodings
                        for encoding in ['utf-8', 'gbk', 'gb2312', 'latin-1']:
                            try:
                                markdown_content = raw_data.decode(encoding)
                                break
                            except (UnicodeDecodeError, ValueError):
                                continue
                        else:
                            markdown_content = raw_data.decode('utf-8', errors='replace')
                elif '/' in file_info.filename:
                    # Extract image files
                    filename = os.path.basename(file_info.filename)
                    if filename and any(filename.lower().endswith(ext) for ext in ['.png', '.jpg', '.jpeg', '.gif', '.svg']):
                        with zip_ref.open(file_info) as img_file:
                            img_data = img_file.read()
                            img_path = images_dir / filename
                            with open(img_path, 'wb') as f:
                                f.write(img_data)
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
            raise Exception("Invalid response format from MinerU")

    if not markdown_content:
        raise Exception("No markdown content found in MinerU result")

    # Save markdown
    md_path = output_path / "output.md"
    with open(md_path, "w", encoding="utf-8") as f:
        f.write(markdown_content)

    return str(md_path)
