import os
from pathlib import Path

import httpx
from app.config import get_settings

settings = get_settings()


async def parse_pdf(file_path: str, output_dir: str) -> str:
    """
    Call MinerU API to parse PDF into Markdown.
    Returns the path to the generated markdown file.
    """
    output_path = Path(output_dir)
    output_path.mkdir(parents=True, exist_ok=True)

    # Read the PDF file
    with open(file_path, "rb") as f:
        pdf_content = f.read()

    # Call MinerU API
    async with httpx.AsyncClient(timeout=300.0) as client:
        # Upload file and start parsing
        files = {"file": (os.path.basename(file_path), pdf_content, "application/pdf")}
        headers = {}
        if settings.mineru_key:
            headers["Authorization"] = f"Bearer {settings.mineru_key}"

        response = await client.post(
            f"{settings.mineru_url}/predict",
            files=files,
            headers=headers,
            data={"task": "parse"},
        )
        response.raise_for_status()
        result = response.json()

    # Extract markdown content from response
    # MinerU typically returns results in a specific format
    markdown_content = ""
    if isinstance(result, dict):
        # Try different response formats
        if "markdown" in result:
            markdown_content = result["markdown"]
        elif "content" in result:
            markdown_content = result["content"]
        elif "result" in result:
            if isinstance(result["result"], dict):
                markdown_content = result["result"].get("markdown", str(result["result"]))
            else:
                markdown_content = str(result["result"])
        else:
            markdown_content = str(result)
    else:
        markdown_content = str(result)

    # Save markdown
    md_path = output_path / "output.md"
    with open(md_path, "w", encoding="utf-8") as f:
        f.write(markdown_content)

    return str(md_path)
