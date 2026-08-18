import os
import requests
from pydantic import BaseModel, Field


class Tools:
    def __init__(self):
        # OKB-Assist 服务配置
        self.base_url = os.getenv("OKB_ASSIST_URL", "http://192.168.1.183:5001")
        self.token = os.getenv("OKB_ASSIST_TOKEN", "")

    def _get_headers(self) -> dict:
        """获取请求头"""
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["X-Token"] = self.token
        return headers

    def search_knowledge_base(
        self,
        query: str = Field(
            ..., description="搜索查询内容，用于在知识库中进行语义搜索"
        ),
        limit: int = Field(
            5, description="返回结果数量，默认为5"
        ),
    ) -> str:
        """
        在 OKB-Assist 知识库中搜索文献资料。

        使用语义搜索技术，根据查询内容查找最相关的文献片段。
        返回结果包含文献标题、作者、年份、期刊以及匹配的文本内容。
        """

        if not query.strip():
            return "错误：搜索查询不能为空"

        try:
            response = requests.get(
                f"{self.base_url}/assist/api/documents/search",
                params={"q": query, "limit": limit},
                headers=self._get_headers(),
                timeout=30,
            )
            response.raise_for_status()
            data = response.json()

            results = data.get("results", [])
            if not results:
                return f"未找到与 \"{query}\" 相关的文献资料"

            # 格式化输出
            output_lines = [f"搜索 \"{query}\" 找到 {len(results)} 条相关结果：\n"]

            for i, result in enumerate(results, 1):
                output_lines.append(f"--- 结果 {i} ---")

                # 文献基本信息
                title = result.get("title", "未知标题")
                authors = result.get("authors", "")
                year = result.get("year", "")
                journal = result.get("journal", "")
                filename = result.get("filename", "")

                if title:
                    output_lines.append(f"标题：{title}")
                if authors:
                    output_lines.append(f"作者：{authors}")
                if year:
                    output_lines.append(f"年份：{year}")
                if journal:
                    output_lines.append(f"期刊：{journal}")
                if filename:
                    output_lines.append(f"文件：{filename}")

                # 匹配的文本内容
                content = result.get("content", "")
                if content:
                    # 截断过长的内容
                    if len(content) > 500:
                        content = content[:500] + "..."
                    output_lines.append(f"内容：{content}")

                # 相似度分数
                score = result.get("score")
                if score is not None:
                    output_lines.append(f"相关度：{score:.2%}")

                output_lines.append("")  # 空行分隔

            return "\n".join(output_lines)

        except requests.exceptions.ConnectionError:
            return f"错误：无法连接到 OKB-Assist 服务 ({self.base_url})"
        except requests.exceptions.Timeout:
            return "错误：请求超时，请稍后重试"
        except requests.exceptions.RequestException as e:
            return f"错误：请求失败 - {str(e)}"

    def list_documents(
        self,
        status: str = Field(
            "indexed",
            description="文档状态过滤：uploaded/parsing/markdown_done/extracting/meta_done/indexing/indexed/error，留空表示全部",
        ),
        limit: int = Field(
            10, description="返回结果数量，默认为10"
        ),
    ) -> str:
        """
        列出知识库中的文献资料。

        可以按状态过滤文献，返回文献的基本信息列表。
        """

        try:
            params = {"page_size": limit}
            if status:
                params["status_filter"] = status

            response = requests.get(
                f"{self.base_url}/assist/api/documents/",
                params=params,
                headers=self._get_headers(),
                timeout=30,
            )
            response.raise_for_status()
            data = response.json()

            items = data.get("items", [])
            total = data.get("total", 0)

            if not items:
                status_text = f"状态为 {status} 的" if status else ""
                return f"知识库中没有{status_text}文献"

            # 状态标签映射
            status_labels = {
                "uploaded": "已上传",
                "parsing": "解析中",
                "markdown_done": "已解析",
                "extracting": "提取中",
                "meta_done": "已提取",
                "indexing": "索引中",
                "indexed": "已索引",
                "error": "错误",
            }

            output_lines = [f"知识库文献列表（共 {total} 篇，显示前 {len(items)} 篇）：\n"]

            for i, doc in enumerate(items, 1):
                title = doc.get("title", doc.get("filename", "未知标题"))
                authors = doc.get("authors", "")
                year = doc.get("year", "")
                doc_status = doc.get("status", "")
                doc_type = doc.get("doc_type", "")

                status_text = status_labels.get(doc_status, doc_status)

                output_lines.append(f"{i}. {title}")
                if authors:
                    output_lines.append(f"   作者：{authors}")
                if year:
                    output_lines.append(f"   年份：{year}")
                if doc_type:
                    output_lines.append(f"   类型：{doc_type}")
                output_lines.append(f"   状态：{status_text}")
                output_lines.append("")

            return "\n".join(output_lines)

        except requests.exceptions.ConnectionError:
            return f"错误：无法连接到 OKB-Assist 服务 ({self.base_url})"
        except requests.exceptions.Timeout:
            return "错误：请求超时，请稍后重试"
        except requests.exceptions.RequestException as e:
            return f"错误：请求失败 - {str(e)}"

    def get_document_detail(
        self,
        doc_id: int = Field(
            ..., description="文献ID"
        ),
    ) -> str:
        """
        获取指定文献的详细信息。

        返回文献的完整元数据，包括标题、作者、摘要、DOI等信息。
        """

        try:
            response = requests.get(
                f"{self.base_url}/assist/api/documents/{doc_id}",
                headers=self._get_headers(),
                timeout=30,
            )
            response.raise_for_status()
            doc = response.json()

            output_lines = [f"文献详情（ID: {doc_id}）：\n"]

            # 基本信息
            title = doc.get("title", "")
            if title:
                output_lines.append(f"标题：{title}")

            title_en = doc.get("title_en", "")
            if title_en:
                output_lines.append(f"英文标题：{title_en}")

            authors = doc.get("authors", "")
            if authors:
                output_lines.append(f"作者：{authors}")

            authors_en = doc.get("authors_en", "")
            if authors_en:
                output_lines.append(f"英文作者：{authors_en}")

            year = doc.get("year", "")
            if year:
                output_lines.append(f"年份：{year}")

            doc_type = doc.get("doc_type", "")
            if doc_type:
                output_lines.append(f"类型：{doc_type}")

            language = doc.get("language", "")
            if language:
                output_lines.append(f"语言：{language}")

            # 出版信息
            journal = doc.get("journal", "")
            if journal:
                output_lines.append(f"期刊：{journal}")

            journal_en = doc.get("journal_en", "")
            if journal_en:
                output_lines.append(f"英文期刊：{journal_en}")

            doi = doc.get("doi", "")
            if doi:
                output_lines.append(f"DOI：{doi}")

            source = doc.get("source", "")
            if source:
                output_lines.append(f"来源：{source}")

            # 内容信息
            abstract = doc.get("abstract", "")
            if abstract:
                output_lines.append(f"\n摘要：\n{abstract}")

            abstract_en = doc.get("abstract_en", "")
            if abstract_en:
                output_lines.append(f"\n英文摘要：\n{abstract_en}")

            keywords = doc.get("keywords", "")
            if keywords:
                output_lines.append(f"\n关键词：{keywords}")

            keywords_en = doc.get("keywords_en", "")
            if keywords_en:
                output_lines.append(f"英文关键词：{keywords_en}")

            category = doc.get("category", "")
            if category:
                output_lines.append(f"分类：{category}")

            return "\n".join(output_lines)

        except requests.exceptions.ConnectionError:
            return f"错误：无法连接到 OKB-Assist 服务 ({self.base_url})"
        except requests.exceptions.Timeout:
            return "错误：请求超时，请稍后重试"
        except requests.exceptions.RequestException as e:
            if "404" in str(e):
                return f"错误：未找到ID为 {doc_id} 的文献"
            return f"错误：请求失败 - {str(e)}"

    def get_document_links(
        self,
        doc_id: int = Field(
            ..., description="文献ID"
        ),
    ) -> str:
        """
        获取指定文献的所有相关链接。

        返回文献详情页、PDF下载、Markdown内容等链接。
        """

        # 去掉末尾的斜杠
        base_url = self.base_url.rstrip("/")

        # 首先验证文献是否存在
        try:
            response = requests.get(
                f"{base_url}/assist/api/documents/{doc_id}",
                headers=self._get_headers(),
                timeout=30,
            )
            response.raise_for_status()
            doc = response.json()
        except requests.exceptions.ConnectionError:
            return f"错误：无法连接到 OKB-Assist 服务 ({self.base_url})"
        except requests.exceptions.Timeout:
            return "错误：请求超时，请稍后重试"
        except requests.exceptions.RequestException as e:
            if "404" in str(e):
                return f"错误：未找到ID为 {doc_id} 的文献"
            return f"错误：请求失败 - {str(e)}"

        title = doc.get("title", doc.get("filename", "未知标题"))
        status = doc.get("status", "")

        # 状态标签映射
        status_labels = {
            "uploaded": "已上传",
            "parsing": "解析中",
            "markdown_done": "已解析",
            "extracting": "提取中",
            "meta_done": "已提取",
            "indexing": "索引中",
            "indexed": "已索引",
            "error": "错误",
        }

        output_lines = [
            f"文献链接信息（ID: {doc_id}）",
            f"标题：{title}",
            f"状态：{status_labels.get(status, status)}",
            "",
            "相关链接：",
        ]

        # 详情页面链接
        detail_url = f"{base_url}/assist/detail/{doc_id}"
        output_lines.append(f"📄 详情页面：{detail_url}")

        # PDF 链接
        pdf_url = f"{base_url}/assist/api/documents/{doc_id}/pdf"
        output_lines.append(f"📕 PDF 下载：{pdf_url}")

        # Markdown 内容链接（始终显示，即使文件尚未生成）
        markdown_url = f"{base_url}/assist/api/documents/{doc_id}/markdown"
        output_lines.append(f"📝 Markdown 内容：{markdown_url}")

        output_lines.append("")
        output_lines.append("使用说明：")
        output_lines.append("- 详情页面：在浏览器中打开可查看文献完整信息")
        output_lines.append("- PDF 下载：直接下载或在浏览器中查看 PDF 文件")
        output_lines.append("- Markdown 内容：获取文献的 Markdown 格式内容（JSON 格式返回）")

        return "\n".join(output_lines)
