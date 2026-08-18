"""从 system.json 推导文档相关路径（不再入库存储）。

system.json 中定义了带 {id} 占位符的路径模板：
- markdown_path       : markdown 正文，如 /.../markdowns/{id}.md
- info_path           : 元信息 json，如 /.../markdowns/{id}.json
- pdf_path            : 源 PDF，如 /.../pdfs/{id}/{id}.pdf
- markdown_asset_path : 图片资源 zip，如 /.../pdfs/{id}/{id}.zip

所有模板均为绝对路径，按 doc id 替换 {id} 即可。
"""
from app.config_manager import load_system_config


def _resolve(template: str | None, doc_id: int) -> str:
    """将模板中的 {id} 替换为文档 id，返回绝对路径。"""
    if not template:
        return ""
    return template.replace("{id}", str(doc_id))


def get_markdown_path(doc_id: int) -> str:
    """文档 markdown 正文的绝对路径。"""
    return _resolve(load_system_config().get("markdown_path"), doc_id)


def get_info_path(doc_id: int) -> str:
    """文档元信息 json 的绝对路径。"""
    return _resolve(load_system_config().get("info_path"), doc_id)


def get_pdf_path(doc_id: int) -> str:
    """文档源 PDF 的绝对路径。"""
    return _resolve(load_system_config().get("pdf_path"), doc_id)


def get_asset_path(doc_id: int) -> str:
    """文档图片资源 zip 的绝对路径（markdown_asset_path）。"""
    return _resolve(load_system_config().get("markdown_asset_path"), doc_id)


def get_crossref_path(doc_id: int) -> str:
    """文档 Crossref 原始返回 JSON 的绝对路径（crossref_path）。

    模板在 system.json 中以 ``{id}`` 占位符定义，替换后得到
    ``/.../markdowns/{id}_crossref.json`` 之类的路径。
    """
    return _resolve(load_system_config().get("crossref_path"), doc_id)
