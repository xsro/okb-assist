import uuid
from typing import Optional

from qdrant_client import QdrantClient
from qdrant_client.models import Distance, VectorParams, PointStruct

from app.config import get_settings

settings = get_settings()

VECTOR_SIZE = 768  # nomic-embed-text default dimension

# Cached client to avoid repeated connections
_client: Optional[QdrantClient] = None
_collection_cache: set[str] = set()


def get_qdrant_client() -> QdrantClient:
    """Get Qdrant client instance (cached)."""
    global _client
    if _client is None:
        _client = QdrantClient(url=settings.qdrant_url)
    return _client


def ensure_collection(client: QdrantClient, collection_name: str):
    """Create collection if it doesn't exist. Uses cache to avoid repeated API calls."""
    global _collection_cache
    if collection_name in _collection_cache:
        return

    collections = client.get_collections().collections
    existing = {c.name for c in collections}

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

    _collection_cache.add(collection_name)


def chunk_text(text: str, chunk_size: int = 512, overlap: int = 50) -> list[str]:
    """
    Split text into chunks by paragraphs, with fallback to character-based chunking.
    Overlap is applied between chunks to preserve context at boundaries.
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

    # Apply overlap: prepend the end of the previous chunk to the start of the next
    if overlap > 0 and len(final_chunks) > 1:
        overlapped = [final_chunks[0]]
        for i in range(1, len(final_chunks)):
            prev = final_chunks[i - 1]
            # Take last `overlap` characters, break at word boundary
            overlap_text = prev[-overlap:]
            space_idx = overlap_text.find(' ')
            if space_idx != -1:
                overlap_text = overlap_text[space_idx + 1:]
            overlapped.append(overlap_text + " " + final_chunks[i])
        final_chunks = overlapped

    return final_chunks


async def get_embeddings_batch(texts: list[str], get_embedding_func) -> list[list[float]]:
    """
    Get embeddings for multiple texts in a single batch request.
    Falls back to individual requests if batch fails.
    """
    try:
        return await get_embedding_func(texts)
    except Exception:
        # Fallback to individual requests
        results = []
        for text in texts:
            emb = await get_embedding_func(text)
            results.append(emb)
        return results


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

    Optimizations:
    - Deletes existing points before re-indexing to avoid duplicates
    - Uses batch embedding to reduce HTTP requests
    - Caches Qdrant client and collection existence checks
    """
    collection_name = f"{settings.qdrant_collection}_{user_id}"

    client = get_qdrant_client()
    ensure_collection(client, collection_name)

    # Delete existing points for this document (prevents duplicates on re-index)
    delete_document_points(user_id, doc_id)

    # Chunk the markdown
    chunks = chunk_text(markdown_content)
    if not chunks:
        return collection_name

    # Generate embeddings in batch
    embeddings = await get_embeddings_batch(chunks, get_embedding_func)

    # Build points
    points = []
    for i, (chunk, embedding) in enumerate(zip(chunks, embeddings)):
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

    # Check if collection exists (use cache)
    global _collection_cache
    if collection_name not in _collection_cache:
        collections = client.get_collections().collections
        existing = {c.name for c in collections}
        _collection_cache = existing
        if collection_name not in existing:
            return []

    embedding = await get_embedding_func(query)
    if not embedding:
        return []

    results = client.query_points(
        collection_name=collection_name,
        query=embedding,
        using="vector",
        limit=limit,
    )

    return [
        {
            "score": hit.score,
            "document_id": hit.payload.get("metadata", {}).get("document_id"),
            "chunk_text": hit.payload.get("text"),
            "title": hit.payload.get("metadata", {}).get("title"),
        }
        for hit in results.points
    ]


def get_point(point_id: str, collection_name: str = None) -> dict:
    """
    Get a point by ID from Qdrant.
    Returns point data or None if not found.
    """
    if collection_name is None:
        collection_name = f"{settings.qdrant_collection}_0"

    client = get_qdrant_client()

    try:
        # Use scroll to get point with vector
        result = client.scroll(
            collection_name=collection_name,
            scroll_filter={
                "must": [
                    {"key": "id", "match": {"value": point_id}}
                ]
            },
            limit=1,
            with_payload=True,
            with_vectors=True,
        )

        points = result[0] if result else []
        if points:
            point = points[0]
            return {
                "id": point.id,
                "payload": point.payload,
                "vector": point.vector,
            }

        # Try direct retrieve as fallback
        points = client.retrieve(
            collection_name=collection_name,
            ids=[point_id],
            with_payload=True,
        )
        if points:
            point = points[0]
            return {
                "id": point.id,
                "payload": point.payload,
                "vector": None,
            }
    except Exception as e:
        return {"error": str(e)}

    return None


def list_collections() -> list[str]:
    """List all collections in Qdrant."""
    client = get_qdrant_client()
    try:
        collections = client.get_collections().collections
        return [c.name for c in collections]
    except Exception:
        return []
