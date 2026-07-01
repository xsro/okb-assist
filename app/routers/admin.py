from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.orm import Session

from app.database import get_db
from app.models import Document, DocStatus
from app.services.qdrant import get_qdrant_client, get_point, list_collections

router = APIRouter(prefix="/assist/api/admin", tags=["admin"])


@router.get("/stats")
def get_stats(db: Session = Depends(get_db)):
    doc_count = db.query(Document).count()

    # Count by status
    status_counts = {}
    for s in DocStatus:
        count = db.query(Document).filter(Document.status == s).count()
        status_counts[s.value] = count

    # Check Qdrant connection
    qdrant_status = "connected"
    qdrant_collections = []
    try:
        client = get_qdrant_client()
        collections = client.get_collections().collections
        qdrant_collections = [c.name for c in collections]
    except Exception as e:
        qdrant_status = f"error: {str(e)}"

    return {
        "documents": doc_count,
        "status_counts": status_counts,
        "qdrant_status": qdrant_status,
        "qdrant_collections": qdrant_collections,
    }


@router.get("/qdrant/collections")
def get_collections():
    """List all Qdrant collections."""
    collections = list_collections()
    return {"collections": collections}


@router.get("/qdrant/point/{point_id}")
def get_point_data(point_id: str, collection: str = None):
    """Get point data from Qdrant by ID."""
    result = get_point(point_id, collection)
    if result is None:
        raise HTTPException(status_code=404, detail="Point not found")
    if "error" in result:
        raise HTTPException(status_code=500, detail=result["error"])
    return result
