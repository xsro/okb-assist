#!/usr/bin/env python3
"""Enrich documents that have an empty title but a valid DOI, using Crossref.

For each document in `documents` whose `title` is empty (and which has a valid
DOI in the `doi` column), look up full metadata from Crossref by DOI, save the
raw Crossref work item to `/home/orangepi/sys/okb-knowledge/markdowns/{id}_crossref.json`,
and UPDATE the DB row with the resolved fields.

The Crossref interaction (endpoint, polite-pool headers, markup stripping,
author string building, and doc_type mapping) mirrors `recover_round2.py`.

Default behavior is APPLY (write JSON + UPDATE DB). Use --dry-run to preview
the parsed metadata without writing anything.

Usage:
    .venv/bin/python scripts/enrich_empty_title_via_crossref.py --dry-run
    .venv/bin/python scripts/enrich_empty_title_via_crossref.py
"""
import argparse
import html
import json
import os
import re
import sqlite3
import time

import httpx

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
DB_PATH = "/home/orangepi/sys/okb-assist/okb_assist.db"
MARKDOWNS_DIR = "/home/orangepi/sys/okb-knowledge/markdowns"

CROSSREF = "https://api.crossref.org/works"
MAILTO = "okb-recover@example.com"
UA = "okb-assist/1.0 (mailto:okb-recover@example.com)"

# Expected target id set (empty-title records with a valid DOI), per the task.
EXPECTED_IDS = [174, 195, 277, 361, 542, 637, 648, 1243, 1326, 1373]

DOI_RE = re.compile(r"^10\.\d{4,9}/.+$")

# ---------------------------------------------------------------------------
# Reused helpers (mirrors recover_round2.py)
# ---------------------------------------------------------------------------
def strip_markup(t):
    """Remove Crossref/HTML markup but KEEP the text inside math tags
    (e.g. <tex-math>$Q$</tex-math> -> Q) so content isn't destroyed."""
    if not t:
        return ""
    t = re.sub(r"<tex-math[^>]*>(.*?)</tex-math>", r" \1 ", t, flags=re.S | re.I)
    t = re.sub(r"<mml:math[^>]*>(.*?)</mml:math>", r" \1 ", t, flags=re.S | re.I)
    t = re.sub(r"<[^>]+>", "", t)          # drop remaining tags
    t = re.sub(r"\$([^$]*)\$", r"\1", t)   # drop LaTeX $ delimiters, keep body
    t = t.replace("$_{\\infty}$", "∞").replace("$_{∞}$", "∞")
    return t


def crossref_authors(item):
    """Return a list of author display strings.

    given+family when both present, else family, else given, else name.
    """
    out = []
    for a in item.get("author", []) or []:
        name = (a.get("name") or "").strip()
        if name:
            out.append(name)
            continue
        given = (a.get("given") or "").strip()
        family = (a.get("family") or "").strip()
        if given and family:
            out.append(f"{given} {family}")
        elif family:
            out.append(family)
        elif given:
            out.append(given)
    return out


def doc_type_map(t):
    """Map a Crossref `type` to this project's canonical doc_type vocabulary.

    Reproduces recover_round2.py's canonical mapping (article / conference /
    book / book_chapter / preprint -- the dominant values in the DB) and adds
    the thesis/dissertation and conference-paper cases enumerated in the task.
    """
    return {
        "journal-article": "article",
        "proceedings-article": "conference",
        "proceedings": "conference",
        "conference-paper": "conference",
        "book": "book",
        "book-chapter": "book_chapter",
        "thesis": "thesis",
        "dissertation": "thesis",
        "posted": "preprint",
        "posted-content": "preprint",
    }.get(t, t)


def normalize_doi(raw):
    """Minimal DOI cleanup: strip whitespace and a known URL/`doi:` prefix."""
    if not raw:
        return None
    s = str(raw).strip()
    for prefix in ("https://doi.org/", "http://doi.org/",
                   "https://", "http://", "doi:"):
        if s.lower().startswith(prefix):
            s = s[len(prefix):].strip()
            break
    if DOI_RE.match(s):
        return s
    return None


# ---------------------------------------------------------------------------
# DB access
# ---------------------------------------------------------------------------
def get_targets():
    con = sqlite3.connect(DB_PATH)
    con.row_factory = sqlite3.Row
    try:
        cur = con.cursor()
        rows = cur.execute(
            "SELECT id, doi, filename FROM documents "
            "WHERE title IS NULL OR trim(title)=''"
        ).fetchall()
    finally:
        con.close()
    return [dict(r) for r in rows]


# ---------------------------------------------------------------------------
# Crossref lookup
# ---------------------------------------------------------------------------
def lookup_crossref(client, doi):
    """Return (message_dict, error_or_None). On non-200 or empty message,
    returns (None, error_string)."""
    try:
        r = client.get(f"{CROSSREF}/{doi}", params={"mailto": MAILTO})
    except Exception as e:
        return None, f"request error: {e}"
    if r.status_code != 200:
        return None, f"HTTP {r.status_code}"
    try:
        data = r.json()
    except Exception as e:
        return None, f"JSON decode error: {e}"
    message = data.get("message")
    if not message:
        return None, "empty message"
    return message, None


def parse_item(message):
    """Parse the Crossref work item into a dict of DB-ready values."""
    title = ""
    raw_title = message.get("title")
    if isinstance(raw_title, list) and raw_title:
        title = strip_markup(html.unescape(raw_title[0])).strip()

    authors = crossref_authors(message)
    authors_str = json.dumps(authors, ensure_ascii=False)

    year = None
    try:
        dp = message.get("issued", {}).get("date-parts")
        if dp and dp[0] and dp[0][0]:
            year = int(dp[0][0])
    except Exception:
        year = None

    journal = ""
    ct = message.get("container-title")
    if isinstance(ct, list) and ct:
        journal = html.unescape(ct[0]).strip()

    doi = message.get("DOI") or ""

    abstract = ""
    ab = message.get("abstract")
    if ab:
        abstract = strip_markup(html.unescape(ab)).strip()

    doc_type = doc_type_map(message.get("type"))

    return {
        "title": title,
        "authors": authors_str,
        "year": year,
        "journal": journal,
        "doi": doi,
        "abstract": abstract,
        "doc_type": doc_type,
    }


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dry-run", action="store_true",
                    help="Preview parsed metadata; do NOT write JSON or UPDATE DB.")
    args = ap.parse_args()
    dry_run = args.dry_run

    print("=" * 78)
    print("ENRICH EMPTY-TITLE DOCS VIA CROSSREF — "
          + ("DRY-RUN (no writes)" if dry_run else "APPLY (write JSON + UPDATE DB)"))
    print("=" * 78)

    targets = get_targets()
    found_ids = sorted(t["id"] for t in targets)
    print(f"Records with empty title: {len(targets)}  ids={found_ids}")

    # Sanity-check the id set against the expected 10.
    if set(found_ids) != set(EXPECTED_IDS):
        extra = sorted(set(found_ids) - set(EXPECTED_IDS))
        missing = sorted(set(EXPECTED_IDS) - set(found_ids))
        print("[SANITY] id set DIFFERS from expected 10.")
        if extra:
            print(f"  unexpected extra ids: {extra}")
        if missing:
            print(f"  missing expected ids: {missing}")
        print("STOPPING before any writes (per task guard).")
        return
    else:
        print("[SANITY] id set matches the expected 10. Proceeding.")

    # Validate DOIs up front; skip malformed ones before hitting the network.
    valid_targets = []
    for t in targets:
        nd = normalize_doi(t["doi"])
        if not nd:
            print(f"[SKIP] id={t['id']} malformed DOI: {t['doi']!r}")
            continue
        t["_doi"] = nd
        valid_targets.append(t)

    results = []  # dict per record: id, doi, ok, parsed, raw_message, error

    with httpx.Client(timeout=30, headers={"User-Agent": UA}) as client:
        for i, t in enumerate(valid_targets):
            cid = t["id"]
            doi = t["_doi"]
            message, err = lookup_crossref(client, doi)
            if err:
                print(f"[FAIL] id={cid} doi={doi} : {err}")
                results.append({"id": cid, "doi": doi, "ok": False, "error": err})
                if i < len(valid_targets) - 1:
                    time.sleep(1)
                continue
            parsed = parse_item(message)
            results.append({
                "id": cid, "doi": doi, "ok": True,
                "parsed": parsed, "raw_message": message,
            })
            print(f"[OK]   id={cid} doi={doi} : title={parsed['title'][:60]!r}")
            if i < len(valid_targets) - 1:
                time.sleep(1)

    # ----- WRITE PHASE (skipped entirely in dry-run) -----
    if not dry_run:
        os.makedirs(MARKDOWNS_DIR, exist_ok=True)
        con = sqlite3.connect(DB_PATH)
        try:
            cur = con.cursor()
            update_sql = (
                "UPDATE documents SET title=?, authors=?, year=?, journal=?, "
                "doi=?, abstract=?, doc_type=?, title_en=?, authors_en=?, "
                "journal_en=?, abstract_en=?, updated_at=CURRENT_TIMESTAMP "
                "WHERE id=?"
            )
            for res in results:
                if not res["ok"]:
                    continue
                cid = res["id"]
                p = res["parsed"]
                # raw Crossref JSON
                json_path = os.path.join(MARKDOWNS_DIR, f"{cid}_crossref.json")
                with open(json_path, "w", encoding="utf-8") as f:
                    json.dump(res["raw_message"], f, ensure_ascii=False, indent=2)
                # DB update
                cur.execute(update_sql, (
                    p["title"], p["authors"], p["year"], p["journal"],
                    p["doi"], p["abstract"], p["doc_type"],
                    p["title"], p["authors"], p["journal"], p["abstract"],
                    cid,
                ))
                con.commit()
        finally:
            con.close()

    # ----- REPORT -----
    print("\n" + "-" * 78)
    print("RESULT TABLE")
    print("-" * 78)
    hdr = f"{'id':>6} | {'doi':<32} | {'resolved':<8} | title / journal / year / json"
    print(hdr)
    for res in results:
        cid = res["id"]
        doi = res["doi"]
        if not res["ok"]:
            print(f"{cid:>6} | {doi:<32} | {'NO':<8} | ERROR: {res['error']}")
            continue
        p = res["parsed"]
        json_size = ""
        if not dry_run:
            json_path = os.path.join(MARKDOWNS_DIR, f"{cid}_crossref.json")
            json_size = f"{os.path.getsize(json_path)}B"
        print(f"{cid:>6} | {doi:<32} | {'YES':<8} | {p['title'][:50]}")
        print(f"{'':>6} | {'':<32} | {'':<8} | journal={p['journal'][:40]!r} "
              f"year={p['year']} json={json_size}")

    # ----- VERIFICATION (post-apply) -----
    if not dry_run:
        print("\n" + "-" * 78)
        print("VERIFICATION (re-query DB)")
        print("-" * 78)
        con = sqlite3.connect(DB_PATH)
        con.row_factory = sqlite3.Row
        try:
            cur = con.cursor()
            rows = {r["id"]: dict(r) for r in cur.execute(
                "SELECT id, title, journal, year, doi FROM documents "
                f"WHERE id IN ({','.join(str(i) for i in EXPECTED_IDS)})"
            )}
        finally:
            con.close()

        for cid in EXPECTED_IDS:
            row = rows.get(cid, {})
            new_title = (row.get("title") or "").strip()
            status = "non-empty" if new_title else "STILL EMPTY"
            json_path = os.path.join(MARKDOWNS_DIR, f"{cid}_crossref.json")
            json_exists = os.path.exists(json_path)
            json_size = f"{os.path.getsize(json_path)}B" if json_exists else "MISSING"
            print(f"  id={cid}: title {status} -> {new_title[:55]!r} "
                  f"| journal={row.get('journal')!r} year={row.get('year')} "
                  f"| json={json_size}")

        missing_json = [cid for cid in EXPECTED_IDS
                        if not os.path.exists(os.path.join(MARKDOWNS_DIR, f"{cid}_crossref.json"))]
        still_empty = [cid for cid in EXPECTED_IDS
                       if not (rows.get(cid, {}).get("title") or "").strip()]
        failed = [res["id"] for res in results if not res["ok"]]
        print("\n  Summary:")
        print(f"    records with still-empty title : {still_empty or 'none'}")
        print(f"    missing JSON files             : {missing_json or 'none'}")
        print(f"    Crossref failures (skipped)    : {failed or 'none'}")

    print("\nDone.")


if __name__ == "__main__":
    main()
