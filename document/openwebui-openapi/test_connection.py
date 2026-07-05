#!/usr/bin/env python3
"""
测试 OKB-Assist OpenAPI 连接
"""

import requests
import json
import sys

def test_openapi_schema(base_url: str):
    """测试 OpenAPI schema 是否可访问"""
    print(f"测试 OpenAPI Schema: {base_url}/openapi.json")

    try:
        response = requests.get(f"{base_url}/openapi.json", timeout=5)
        response.raise_for_status()
        schema = response.json()

        print(f"✅ 成功获取 OpenAPI Schema")
        print(f"   Title: {schema.get('info', {}).get('title')}")
        print(f"   Version: {schema.get('info', {}).get('version')}")

        # 检查是否有 openapi 路由
        paths = schema.get('paths', {})
        openapi_paths = [p for p in paths.keys() if '/openapi/' in p]

        if openapi_paths:
            print(f"✅ 找到 {len(openapi_paths)} 个 OpenAPI 工具端点:")
            for path in sorted(openapi_paths):
                methods = list(paths[path].keys())
                print(f"   - {methods[0].upper()} {path}")
        else:
            print("❌ 未找到 OpenAPI 工具端点")
            return False

        return True

    except requests.exceptions.ConnectionError:
        print(f"❌ 无法连接到 {base_url}")
        return False
    except Exception as e:
        print(f"❌ 错误: {e}")
        return False


def test_search_endpoint(base_url: str):
    """测试搜索端点"""
    print(f"\n测试搜索端点: {base_url}/assist/openapi/search")

    try:
        response = requests.get(
            f"{base_url}/assist/openapi/search",
            params={"q": "test", "limit": 1},
            timeout=10
        )

        if response.status_code == 200:
            data = response.json()
            print(f"✅ 搜索端点正常工作")
            print(f"   查询: {data.get('query')}")
            print(f"   结果数: {data.get('total')}")
            return True
        else:
            print(f"⚠️  返回状态码: {response.status_code}")
            print(f"   响应: {response.text[:200]}")
            return False

    except Exception as e:
        print(f"❌ 错误: {e}")
        return False


def main():
    if len(sys.argv) < 2:
        print("使用方法: python test_connection.py <base_url>")
        print("示例: python test_connection.py http://localhost:8000")
        sys.exit(1)

    base_url = sys.argv[1].rstrip('/')

    print("=" * 50)
    print("OKB-Assist OpenAPI 连接测试")
    print("=" * 50)

    # 测试 OpenAPI Schema
    if not test_openapi_schema(base_url):
        print("\n请检查:")
        print("1. OKB-Assist 服务是否正在运行")
        print("2. URL 是否正确")
        print("3. 端口是否正确")
        sys.exit(1)

    # 测试搜索端点
    test_search_endpoint(base_url)

    print("\n" + "=" * 50)
    print("配置信息")
    print("=" * 50)
    print(f"OpenWebUI 中配置的 URL:")
    print(f"  {base_url}/openapi.json")
    print()
    print("请在 OpenWebUI 中使用上述 URL 配置 OpenAPI Server")


if __name__ == "__main__":
    main()
