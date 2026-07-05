#!/usr/bin/env python3
"""测试元数据提取功能"""

import asyncio
import sys
sys.path.insert(0, '/home/a422/repo/okb-assist')

from app.services.ollama import extract_metadata

async def test():
    # 读取一个错误文档
    with open('/home/a422/repo/okb-assist/uploads/717/output.md', 'r', encoding='utf-8') as f:
        content = f.read()

    print(f"文件大小: {len(content)} 字符")
    print(f"截取前 8000 字符进行测试...")

    try:
        metadata = await extract_metadata(content)
        print(f"提取成功: {metadata}")
    except Exception as e:
        print(f"提取失败: {type(e).__name__}: {e}")

if __name__ == "__main__":
    asyncio.run(test())
