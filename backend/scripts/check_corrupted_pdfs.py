#!/usr/bin/env python3
"""Check SQLite 'documents' rows for missing or corrupted PDFs."""

import csv
import os
import sqlite3
import sys
import traceback

# --- Constants (exact paths) ---
DB_PATH = "/home/orangepi/sys/okb-assist/okb_assist.db"
PDFS_ROOT = "/media/orangepi/CCSICC/okb-knowledge/pdfs"
REPORT_PATH = "/home/orangepi/sys/okb-assist/scripts/corrupted_pdfs_report.csv"
EOF_SCAN_BYTES = 2048


def detect_pdf_library():
    """Return (name, module) for the first available PDF library, or (None, None)."""
    try:
        from pypdf import PdfReader
        return "pypdf", ("pypdf", PdfReader)
    except ImportError:
        pass
    try:
        from PyPDF2 import PdfReader
        return "PyPDF2", ("PyPDF2", PdfReader)
    except ImportError:
        pass
    try:
        import fitz
        return "PyMuPDF", ("fitz", fitz)
    except ImportError:
        pass
    return None, None


def parse_check(path, lib):
    """Return (status, detail). lib is the tuple from detect_pdf_library or None."""
    if lib is None:
        return None, None  # no parse check available
    name, mod = lib
    try:
        if name in ("pypdf", "PyPDF2"):
            reader = mod(path)
            _ = reader.pages
        elif name == "fitz":
            doc = mod.open(path)
            _ = doc.page_count
            doc.close()
        return None, None  # parses fine -> no extra status
    except Exception as e:  # noqa: BLE001
        return "PARSE_ERROR", "{}: {}".format(type(e).__name__, e)


def has_eof(path):
    try:
        with open(path, "rb") as f:
            f.seek(0, os.SEEK_END)
            size = f.tell()
            start = max(0, size - EOF_SCAN_BYTES)
            f.seek(start)
            tail = f.read()
        return b"%%EOF" in tail
    except OSError:
        return False


def classify(path, lib):
    """Classify a PDF file. Returns (status, detail)."""
    if not os.path.exists(path):
        return "MISSING", "file does not exist"
    if not os.path.isfile(path):
        return "MISSING", "path is not a regular file"

    size = os.path.getsize(path)
    if size == 0:
        return "EMPTY", "file is 0 bytes"

    try:
        with open(path, "rb") as f:
            head = f.read(5)
    except OSError as e:
        return "BAD_HEADER", "cannot read header: {}".format(e)

    if head[:5] != b"%PDF-":
        return "BAD_HEADER", "header is {!r}".format(head[:5])

    if not has_eof(path):
        return "NO_EOF", "no %%EOF found in last {} bytes".format(EOF_SCAN_BYTES)

    status, detail = parse_check(path, lib)
    if status is not None:
        return status, detail

    return "OK", ""


def main():
    lib_name, lib = detect_pdf_library()
    print("PDF library detected: {}".format(lib_name if lib_name else "NONE (structural checks only)"), file=sys.stderr)

    conn = sqlite3.connect(DB_PATH)
    conn.row_factory = sqlite3.Row
    try:
        cur = conn.execute("SELECT id, filename, file_hash FROM documents")
        rows = cur.fetchall()
    finally:
        conn.close()

    results = []
    for row in rows:
        doc_id = row["id"]
        filename = row["filename"]
        path = os.path.join(PDFS_ROOT, str(doc_id), "{}.pdf".format(doc_id))
        status, detail = classify(path, lib)
        results.append((doc_id, filename, status, detail))

    # Write CSV report for ALL rows
    with open(REPORT_PATH, "w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(["id", "filename", "status", "detail"])
        for doc_id, filename, status, detail in results:
            writer.writerow([doc_id, filename, status, detail])

    # Summary counts
    counts = {}
    for _, _, status, _ in results:
        counts[status] = counts.get(status, 0) + 1

    total = len(results)
    print("Total documents: {}".format(total))
    print("PDF library: {}".format(lib_name if lib_name else "NONE"))
    print("Report: {}".format(REPORT_PATH))
    print("Status counts:")
    for status in sorted(counts):
        print("  {}: {}".format(status, counts[status]))

    # List problematic (non-OK)
    problems = [(d, fn, st, dt) for (d, fn, st, dt) in results if st != "OK"]
    print("Problematic PDFs ({}):".format(len(problems)))
    for doc_id, filename, status, detail in problems:
        print("[{}] id={}  {}  ({})".format(status, doc_id, filename, detail))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        traceback.print_exc()
        sys.exit(1)
