import hashlib
import os
from pathlib import Path

def get_uploads_folder() -> str:
    """获取 uploads_folder 的绝对路径。"""
    from app.config import get_settings
    settings = get_settings()
    return os.path.abspath(settings.uploads_folder)


def to_relative_path(absolute_path: str) -> str:
    """将绝对路径转换为相对于 uploads_folder 的路径。

    例如: /home/user/uploads/1/1.pdf -> uploads/1/1.pdf
    """
    cwd_folder = os.path.abspath(os.getcwd())
    try:
        return os.path.relpath(absolute_path, cwd_folder)
    except ValueError:
        # Windows 上跨盘符时会报错，返回原路径
        return absolute_path


def to_absolute_path(relative_path: str) -> str:
    """将相对于 uploads_folder 的路径转换为绝对路径。

    例如: uploads/1/1.pdf -> /home/user/uploads/1/1.pdf
    """
    cwd_folder = os.path.abspath(os.getcwd())
    return os.path.join(cwd_folder, relative_path)


def calculate_file_hash(file_path: str) -> str:
    """Calculate SHA256 hash of a file.

    Uses a 4096-byte buffer for memory efficiency.
    This is the canonical hash function shared by server and client scripts.
    """
    sha256_hash = hashlib.sha256()
    with open(file_path, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()
