#!/usr/bin/env python3
"""Test MinerU 4.0 V1 API full flow — keeps results."""
import json, os, sys, time, hashlib, urllib.request, urllib.error

API = "http://192.168.1.185:8002"
PDF = r"F:\liuch586\HOME\Downloads\我国一箭双星成功发射实践九号A_B卫星.pdf"
OUT = r"D:\liuch586\side\okb-assist\test_output"

def req(method, url, data=None):
    h = {"Content-Type": "application/json"}
    body = None if data is None else json.dumps(data).encode("utf-8")
    r = urllib.request.Request(url, data=body, headers=h, method=method)
    try:
        return json.loads(urllib.request.urlopen(r).read())
    except urllib.error.HTTPError as e:
        print(f"  HTTP {e.code}: {e.read().decode('utf-8',errors='replace')[:300]}")
        raise

# 1. Upload
print("=== 1. Upload ===")
basename = os.path.basename(PDF)
file_bytes = os.path.getsize(PDF)
sha256 = hashlib.sha256(open(PDF, "rb").read()).hexdigest()
print(f"  File: {basename}, {file_bytes} bytes, sha256={sha256[:16]}...")

upl = req("POST", f"{API}/v1/uploads", {
    "filename": basename, "bytes": file_bytes,
    "sha256sum": sha256, "mime_type": "application/pdf"
})
upload_id = upl["id"]
upload_url = upl["upload_url"]
upload_headers = upl.get("upload_headers", {})
print(f"  upload_id={upload_id}, url={upload_url}")

# 2. Upload bytes
print("\n=== 2. Upload bytes ===")
full_url = f"{API}{upload_url}" if upload_url.startswith("/") else upload_url
with open(PDF, "rb") as f:
    pdf_data = f.read()
h = {"Content-Type": "application/pdf"}
for k, v in upload_headers.items():
    h[k] = v
put = urllib.request.Request(full_url, data=pdf_data, headers=h, method="PUT")
urllib.request.urlopen(put)
print("  OK")

# 3. Complete upload
print("\n=== 3. Complete ===")
complete = req("POST", f"{API}/v1/uploads/{upload_id}/complete")
file_id = complete["file"]["id"]
print(f"  file_id={file_id}")

# 4. Submit job
print("\n=== 4. Submit job ===")
job = req("POST", f"{API}/v1/parse/jobs", {
    "files": [{"source": {"type": "file_id", "file_id": file_id}}],
    "tier": "standard",
    "output_formats": ["markdown", "zip"],
})
job_id = job["job_id"]
print(f"  job_id={job_id}")

# 5. Poll
print("\n=== 5. Poll ===")
for i in range(120):
    st = req("GET", f"{API}/v1/parse/jobs/{job_id}")
    s = st.get("status", "?")
    print(f"  [{i+1}] {s}")
    if s in ("completed", "partial"):
        break
    if s in ("failed", "canceled"):
        print(f"  FAILED: {st}")
        sys.exit(1)
    time.sleep(2)
else:
    print("  Timed out!"); sys.exit(1)

# 6. Download
os.makedirs(OUT, exist_ok=True)
print(f"\n=== 6. Download to {OUT} ===")
for f in st.get("files", []):
    of = f.get("output_files", {}) or {}
    for fmt, val in of.items():
        if isinstance(val, dict) and val.get("file_id"):
            fid = val["file_id"]
            ext = {"markdown": ".md", "zip": ".zip", "middle_json": ".json"}.get(fmt, "")
            fname = f"output{ext}"
            try:
                resp = urllib.request.urlopen(f"{API}/v1/files/{fid}/content")
                data = resp.read()
                with open(os.path.join(OUT, fname), "wb") as fp:
                    fp.write(data)
                print(f"  {fname} ({fmt}, {len(data)} bytes)")
            except urllib.error.HTTPError as e:
                if e.code in (301, 302, 303, 307, 308):
                    loc = e.headers.get("Location") or e.headers.get("location", "")
                    resp2 = urllib.request.urlopen(loc)
                    data = resp2.read()
                    with open(os.path.join(OUT, fname), "wb") as fp:
                        fp.write(data)
                    print(f"  {fname} ({fmt}, {len(data)} bytes, via redirect)")

print(f"\n=== Output in {OUT} ===")
for fn in sorted(os.listdir(OUT)):
    fp = os.path.join(OUT, fn)
    print(f"  {fn} ({os.path.getsize(fp)} bytes)")

md = os.path.join(OUT, "output.md")
if os.path.exists(md):
    content = open(md, "r", encoding="utf-8").read()
    print(f"\n=== Markdown ({len(content)} chars) ===")
    print(content[:600])
    print("...")
    # Check for embedded images
    img_count = content.count("![](data:image")
    from_refs = content.count("![](images/")
    print(f"\n  Embedded base64 images: {img_count}")
    print(f"  Referenced images/: {from_refs}")