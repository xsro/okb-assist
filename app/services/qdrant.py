import uuid
from typing import Optional

from qdrant_client import QdrantClient
from qdrant_client.models import Distance, VectorParams, PointStruct

from app.config import get_settings

settings = get_settings()

VECTOR_SIZE = 768  # nomic-embed-text default dimension


def get_qdrant_client() -> QdrantClient:
    """Get Qdrant client instance."""
    return QdrantClient(url=settings.qdrant_url)


def ensure_collection(client: QdrantClient, collection_name: str):
    """Create collection if it doesn't exist."""
    collections = client.get_collections().collections
    existing = [c.name for c in collections]

    if collection_name not in existing:
        client.create_collection(
            collection_name=collection_name,
            vectors_config={
                "vector": VectorParams(
                    size=VECTOR_SIZE,
                    distance=Distance.COSINE,
                ),
            },
        )


def chunk_text(text: str, chunk_size: int = 512, overlap: int = 50) -> list[str]:
    """
    Split text into chunks by paragraphs, with fallback to character-based chunking.
    """
    # First try paragraph-based splitting
    paragraphs = text.split("\n\n")
    chunks = []
    current_chunk = ""

    for para in paragraphs:
        para = para.strip()
        if not para:
            continue

        if len(current_chunk) + len(para) < chunk_size:
            current_chunk += ("\n\n" if current_chunk else "") + para
        else:
            if current_chunk:
                chunks.append(current_chunk)
            current_chunk = para

    if current_chunk:
        chunks.append(current_chunk)

    # If chunks are too large, do character-based splitting
    final_chunks = []
    for chunk in chunks:
        if len(chunk) <= chunk_size * 2:
            final_chunks.append(chunk)
        else:
            # Split large chunks
            words = chunk.split()
            sub_chunk = ""
            for word in words:
                if len(sub_chunk) + len(word) + 1 < chunk_size:
                    sub_chunk += (" " if sub_chunk else "") + word
                else:
                    if sub_chunk:
                        final_chunks.append(sub_chunk)
                    sub_chunk = word
            if sub_chunk:
                final_chunks.append(sub_chunk)

    return final_chunks


async def index_document(
    doc_id: int,
    user_id: int,
    markdown_content: str,
    metadata: dict,
    get_embedding_func,
) -> str:
    """
    Index document into Qdrant.
    Returns the collection name.
    """
    collection_name = f"{settings.qdrant_collection}_{user_id}"

    client = get_qdrant_client()
    ensure_collection(client, collection_name)

    # Chunk the markdown
    chunks = chunk_text(markdown_content)

    # Generate embeddings and upload
    points = []
    for i, chunk in enumerate(chunks):
        embedding = await get_embedding_func(chunk)
        if not embedding:
            continue

        point = PointStruct(
            id=str(uuid.uuid4()),
            vector={"vector": embedding},
            payload={
                "text": chunk,
                "metadata": {
                    "document_id": doc_id,
                    "chunk_index": i,
                    "title": metadata.get("title", ""),
                    "authors": metadata.get("authors", []),
                    "year": metadata.get("year"),
                    "doc_type": metadata.get("type", ""),
                    "keywords": metadata.get("keywords", []),
                },
            },
        )
        points.append(point)

    # Upload in batches
    batch_size = 100
    for start in range(0, len(points), batch_size):
        batch = points[start:start + batch_size]
        client.upsert(
            collection_name=collection_name,
            points=batch,
        )

    return collection_name


def delete_document_points(user_id: int, doc_id: int):
    """Delete all points for a document from Qdrant."""
    collection_name = f"{settings.qdrant_collection}_{user_id}"
    client = get_qdrant_client()

    try:
        client.delete(
            collection_name=collection_name,
            points_selector={
                "filter": {
                    "must": [
                        {"key": "metadata.document_id", "match": {"value": doc_id}}
                    ]
                }
            },
        )
    except Exception:
        pass  # Collection might not exist


async def search_similar(
    user_id: int,
    query: str,
    get_embedding_func,
    limit: int = 10,
) -> list[dict]:
    """
    Search for similar chunks in Qdrant.
    """
    collection_name = f"{settings.qdrant_collection}_{user_id}"
    client = get_qdrant_client()

    # Check if collection exists
    collections = client.get_collections().collections
    existing = [c.name for c in collections]
    if collection_name not in existing:
        return []

    embedding = await get_embedding_func(query)
    if not embedding:
        return []

    results = client.search(
        collection_name=collection_name,
        query_vector=("vector", embedding),
        limit=limit,
    )

    return [
        {
            "score": hit.score,
            "document_id": hit.payload.get("metadata", {}).get("document_id"),
            "chunk_text": hit.payload.get("text"),
            "title": hit.payload.get("metadata", {}).get("title"),
        }
        for hit in results
    ]
