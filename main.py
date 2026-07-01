from fastapi import Request
from fastapi.responses import RedirectResponse
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from fastapi import FastAPI

from app.database import init_db
from app.routers import documents, pipeline, admin

app = FastAPI(title="OKB-Assist", description="论文与专著数据管理系统")

# Mount static files
app.mount("/assist/static", StaticFiles(directory="static"), name="static")

# Mount uploads directory for serving images
app.mount("/assist/uploads", StaticFiles(directory="uploads"), name="uploads")

# Templates
templates = Jinja2Templates(directory="app/templates")

# Include routers
app.include_router(documents.router)
app.include_router(pipeline.router)
app.include_router(admin.router)


@app.on_event("startup")
def startup():
    init_db()


@app.get("/")
def root():
    return RedirectResponse(url="/assist/")


@app.get("/assist/")
def index(request: Request):
    return templates.TemplateResponse(name="index.html", request=request)


@app.get("/assist/upload")
def upload_page(request: Request):
    return templates.TemplateResponse(name="upload.html", request=request)


@app.get("/assist/detail/{doc_id}")
def detail_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="detail.html", request=request, context={"doc_id": doc_id})


@app.get("/assist/admin")
def admin_page(request: Request):
    return templates.TemplateResponse(name="admin.html", request=request)


@app.get("/assist/markdown/{doc_id}")
def markdown_page(request: Request, doc_id: int):
    return templates.TemplateResponse(name="markdown.html", request=request, context={"doc_id": doc_id})


if __name__ == "__main__":
    import uvicorn
    uvicorn.run("main:app", host="0.0.0.0", port=5000, reload=True)
