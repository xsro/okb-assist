#!/usr/bin/env python3
"""Recover missing metadata for documents lacking a valid DOI.

For each document in the `documents` table that LACKS a valid DOI, fuzzy-match
its (English/non-CJK) title against infos.csv (Zotero export) and recover
missing fields via the running API. Also write the markdown JSON file from the
matched CSV row when it is missing on disk.

DRY-RUN is the default. Nothing is written or sent unless `--apply` is given.

Usage:
    .venv/bin/python recover_missing_doi.py            # dry-run (preview only)
    .venv/bin/python recover_missing_doi.py --apply     # perform writes + API PUTs
"""

import argparse
import csv
import json
import os
import re
import sqlite3
import sys

import requests

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
DB_PATH = "/home/orangepi/sys/okb-assist/okb_assist.db"
CSV_PATH = "/home/orangepi/sys/okb-knowledge/infos.csv"
MARKDOWNS_DIR = "/home/orangepi/sys/okb-knowledge/markdowns"

API_BASE = "http://localhost:5001"
API_TOKEN = "sysusz422"
API_HEADERS = {"X-Token": API_TOKEN, "Content-Type": "application/json"}

VALID_DOI = re.compile(r"^10\.\d{4,9}/.+$")
CJK_RE = re.compile(r"[\u3000-\u9fff\uff00-\uffef\u3040-\u30ff\uac00-\ud7af]")

RATIO_THRESHOLD = 0.85
SUBSTRING_MIN_LEN = 15

# DB column -> CSV source key (for the simple string fields we recover)
CSV_COLUMNS = [
    "Key", "Item Type", "Title", "Author", "Publication Year",
    "Publication Title", "DOI", "Abstract Note", "Language",
    "Manual Tags", "Automatic Tags",
]

DOC_TYPE_MAP = {
    "journalarticle": "article",
    "conferencepaper": "conference",
    "thesis": "thesis",
    "preprint": "preprint",
    "book": "book",
    "booksection": "book_chapter",
}

# Candidate columns we pull from the DB to know what's currently missing.
CANDIDATE_SELECT = (
    "id, title, authors, year, doi, journal, abstract, language, doc_type, "
    "keywords, title_en, authors_en, abstract_en, journal_en"
)

# The candidate WHERE clause per the spec (an invalid/URL/whitespace DOI is
# still selected; we re-validate with the regex in Python as a hard guard).
CANDIDATE_WHERE = (
    "doi IS NULL OR doi='' OR (doi NOT LIKE '10.%' OR doi NOT LIKE '%/%')"
)


# ---------------------------------------------------------------------------
# Reused helpers (mirrors supplement_db_from_json.py)
# ---------------------------------------------------------------------------
def convert_authors(zotero_author):
    """Convert Zotero 'Author' ("Last, First; Last2, First2") into a
    JSON-array string of 'First Last' form. Returns None if empty."""
    if not zotero_author or not str(zotero_author).strip():
        return None
    parts = [p.strip() for p in str(zotero_author).split(";") if p.strip()]
    names = []
    for part in parts:
        if "," in part:
            last, first = part.split(",", 1)
            name = (first.strip() + " " + last.strip()).strip()
        else:
            name = part
        if name:
            names.append(name)
    if not names:
        return None
    return json.dumps(names, ensure_ascii=False)


def to_year(value):
    if value and str(value).strip():
        try:
            return int(str(value).strip())
        except ValueError:
            return None
    return None


def is_empty(value):
    if value is None:
        return True
    if isinstance(value, str) and value.strip() == "":
        return True
    return False


# Prefixes stripped (case-insensitively) before re-testing a DOI.
_DOI_PREFIXES = ("https://doi.org/", "http://doi.org/",
                 "https://", "http://", "doi:")


def normalize_doi(raw):
    """Deterministic DOI cleanup with NO fuzzy risk.

    If `raw` is not already a valid DOI (per VALID_DOI) but becomes one after
    stripping a known URL/`doi:` prefix and surrounding whitespace, return the
    cleaned DOI. Otherwise return None.

    This recovers clearly-salvageable forms such as
    'https://doi.org/10.x/y' or ' 10.x/y ' without touching genuinely empty
    or real invalid strings.
    """
    if not raw:
        return None
    s = str(raw)
    low = s.lower()
    for prefix in _DOI_PREFIXES:
        if low.startswith(prefix):
            s = s[len(prefix):]
            break
    s = s.strip()
    if re.match(VALID_DOI, s):
        return s
    return None


def decide_doi(db_doi, csv_doi):
    """Return the DOI to PUT for a candidate, or None.

    Priority: a recoverable (prefixed/whitespace) RAW stored DOI wins because
    it is exactly what the user entered; otherwise fall back to a clean CSV DOI
    when the DB doi is missing/invalid and the CSV has a valid one.
    """
    cleaned = normalize_doi(db_doi)
    if cleaned:
        return cleaned
    if csv_doi and re.match(VALID_DOI, csv_doi):
        return csv_doi
    return None


# ---------------------------------------------------------------------------
# Normalization / matching
# ---------------------------------------------------------------------------
def normalize_title(title):
    """lowercase, strip, collapse whitespace, drop a trailing ellipsis."""
    if not title:
        return ""
    s = str(title).strip().lower()
    s = re.sub(r"\s+", " ", s)
    # strip a single trailing ellipsis: …, ..., or ..
    s = re.sub(r"\s*([.]{2,}|\u2026)\s*$", "", s)
    return s.strip()


def convert_keywords(*tag_values):
    """Combine Zotero tag fields (semicolon-separated) into a JSON array."""
    names = []
    for raw in tag_values:
        if not raw:
            continue
        for part in str(raw).split(";"):
            p = part.strip()
            if p and p not in names:
                names.append(p)
    if not names:
        return None
    return json.dumps(names, ensure_ascii=False)


def word_set(title):
    """Set of significant (len>=4) alnum tokens, for a cheap pre-filter."""
    norm = normalize_title(title)
    if not norm:
        return set()
    return {w for w in re.findall(r"[a-z0-9]+", norm) if len(w) >= 4}


def best_match(norm_db_title, csv_index):
    """Return (chosen_row_or_None, score, method).

    `csv_index` is a precomputed list of (row, norm_csv_title, csv_word_set).
    A cheap word-overlap pre-filter avoids running the (expensive) exact
    SequenceMatcher on every row; the exact ratio is still the final score.
    """
    if not norm_db_title:
        return None, 0.0, None
    db_words = word_set_from_norm(norm_db_title)
    best_ratio = 0.0
    best_row = None
    substr_row = None
    substr_ratio = 0.0
    for row, norm_csv, csv_words in csv_index:
        if not norm_csv:
            continue
        # Quick pre-filter: require at least one shared significant word.
        if db_words and csv_words and not (db_words & csv_words):
            continue
        ratio = difflib_ratio(norm_db_title, norm_csv)
        if ratio > best_ratio:
            best_ratio = ratio
            best_row = row
        if len(norm_db_title) > SUBSTRING_MIN_LEN and norm_db_title in norm_csv:
            if ratio > substr_ratio:
                substr_ratio = ratio
                substr_row = row
    if best_ratio >= RATIO_THRESHOLD and best_row is not None:
        return best_row, best_ratio, "ratio"
    if substr_row is not None:
        return substr_row, substr_ratio, "substring"
    return None, best_ratio, None


def word_set_from_norm(norm):
    if not norm:
        return set()
    return {w for w in re.findall(r"[a-z0-9]+", norm) if len(w) >= 4}


def difflib_ratio(a, b):
    # Local import keeps module import light; difflib is stdlib.
    from difflib import SequenceMatcher
    return SequenceMatcher(None, a, b).ratio()


# ---------------------------------------------------------------------------
# Payload building
# ---------------------------------------------------------------------------
def build_payload(db_row, csv_row):
    """Build the PUT payload: only fill DB fields that are empty (except DOI
    which may be fixed from a clean CSV DOI when the DB doi is missing/invalid).
    """
    payload = {}

    # --- DOI: prefer a recoverable (prefixed/whitespace) RAW stored DOI,
    # otherwise fall back to a clean CSV DOI when the DB doi is missing/invalid.
    csv_doi = (csv_row.get("DOI") or "").strip()
    doi = decide_doi(db_row["doi"], csv_doi)
    if doi:
        payload["doi"] = doi

    # --- authors (convert Zotero -> JSON array) ---
    if is_empty(db_row["authors"]):
        authors = convert_authors(csv_row.get("Author"))
        if authors:
            payload["authors"] = authors

    # --- year ---
    if is_empty(db_row["year"]):
        y = to_year(csv_row.get("Publication Year"))
        if y is not None:
            payload["year"] = y

    # --- journal ---
    if is_empty(db_row["journal"]):
        j = (csv_row.get("Publication Title") or "").strip()
        if j:
            payload["journal"] = j

    # --- abstract ---
    if is_empty(db_row["abstract"]):
        a = (csv_row.get("Abstract Note") or "").strip()
        if a:
            payload["abstract"] = a

    # --- language ---
    if is_empty(db_row["language"]):
        lang = (csv_row.get("Language") or "").strip()
        if lang:
            payload["language"] = lang

    # --- doc_type (map Item Type) ---
    if is_empty(db_row["doc_type"]):
        it = (csv_row.get("Item Type") or "").strip().lower()
        if it:
            payload["doc_type"] = DOC_TYPE_MAP.get(it, it)

    # --- keywords (Manual + Automatic Tags) ---
    if is_empty(db_row["keywords"]):
        kw = convert_keywords(csv_row.get("Manual Tags"), csv_row.get("Automatic Tags"))
        if kw:
            payload["keywords"] = kw

    # --- English enrichment: *_en fields ---
    lang = (csv_row.get("Language") or "").strip().lower()
    if lang == "en":
        if is_empty(db_row.get("title_en")) and payload.get("authors") is not None:
            # title_en derived from CSV Title
            csv_title = (csv_row.get("Title") or "").strip()
            if csv_title:
                payload["title_en"] = csv_title
        if is_empty(db_row.get("authors_en")) and payload.get("authors") is not None:
            payload["authors_en"] = payload["authors"]
        if is_empty(db_row.get("abstract_en")) and payload.get("abstract") is not None:
            payload["abstract_en"] = payload["abstract"]
        if is_empty(db_row.get("journal_en")) and payload.get("journal") is not None:
            payload["journal_en"] = payload["journal"]

    return payload


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def load_csv_rows():
    rows = []
    with open(CSV_PATH, encoding="utf-8", newline="") as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append(row)
    return rows


def get_candidates():
    con = sqlite3.connect(DB_PATH)
    con.row_factory = sqlite3.Row
    try:
        cur = con.cursor()
        q = f"SELECT {CANDIDATE_SELECT} FROM documents WHERE {CANDIDATE_WHERE}"
        rows = cur.execute(q).fetchall()
    finally:
        con.close()
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true",
                        help="Actually call the API and write missing JSON files.")
    args = parser.parse_args()
    apply_mode = args.apply

    csv_rows = load_csv_rows()
    # Precompute (row, normalized_title, word_set) for fast matching.
    csv_index = []
    for row in csv_rows:
        nt = normalize_title(row.get("Title"))
        csv_index.append((row, nt, word_set(row.get("Title"))))
    candidates = get_candidates()

    matched = []        # list of dicts: id, db_title, csv_title, score, key, payload, json_to_create
    unmatched = []      # id, db_title
    skipped_cjk = []    # id
    no_title = []       # id

    for c in candidates:
        db_row = dict(c)
        doc_id = db_row["id"]
        title = db_row["title"]

        if not title or not str(title).strip():
            no_title.append(doc_id)
            continue
        if CJK_RE.search(title):
            skipped_cjk.append(doc_id)
            continue

        norm_db = normalize_title(title)
        chosen, score, method = best_match(norm_db, csv_index)
        if chosen is None:
            unmatched.append((doc_id, title))
            continue

        payload = build_payload(db_row, chosen)
        csv_title = chosen.get("Title") or ""
        csv_key = chosen.get("Key") or ""
        json_path = os.path.join(MARKDOWNS_DIR, f"{doc_id}.json")
        json_to_create = not os.path.exists(json_path)

        matched.append({
            "id": doc_id,
            "db_title": title,
            "csv_title": csv_title,
            "score": score,
            "method": method,
            "key": csv_key,
            "payload": payload,
            "json_to_create": json_to_create,
            "matched_row": chosen,  # full ordered row for JSON writing
        })

    # -------------------------------------------------------------------
    # Deterministic DOI-normalization pass (runs over ALL candidates, no
    # fuzzy risk). Recovers clearly-salvageable prefixed/whitespace DOIs.
    # -------------------------------------------------------------------
    cand_by_id = {dict(c)["id"]: dict(c) for c in candidates}
    norm_fixed = {}     # id -> cleaned doi
    for cid, db_row in cand_by_id.items():
        cleaned = normalize_doi(db_row.get("doi"))
        if cleaned:
            norm_fixed[cid] = cleaned

    # CSV-covered DOI ids (the prior path): fuzzy-matched AND the CSV row
    # carries a valid DOI. Used to separate *new* normalizations from those
    # already covered by the CSV path (idempotency for ids like 1251/703).
    csv_doi_ids = {
        m["id"]
        for m in matched
        if re.match(VALID_DOI, (m["matched_row"].get("DOI") or "").strip())
    }
    newly_norm = {i for i in norm_fixed if i not in csv_doi_ids}

    # -------------------------------------------------------------------
    # Build ONE unified set of actions (merge fuzzy match + normalization).
    # -------------------------------------------------------------------
    actions = {}
    for m in matched:
        actions[m["id"]] = {
            "id": m["id"],
            "payload": m["payload"],
            "json_to_create": m["json_to_create"],
            "matched_row": m["matched_row"],
            "source": "fuzzy",
            "db_title": m["db_title"],
            "csv_title": m["csv_title"],
            "score": m["score"],
            "method": m["method"],
            "key": m["key"],
        }
    # Normalization-only actions (candidate not fuzzy-matched): PUT doi only.
    for cid, cleaned in norm_fixed.items():
        if cid in actions:
            # Reflection of decide_doi in build_payload; force for safety.
            actions[cid]["payload"]["doi"] = cleaned
        else:
            db_row = cand_by_id[cid]
            actions[cid] = {
                "id": cid,
                "payload": {"doi": cleaned},
                "json_to_create": False,
                "matched_row": None,
                "source": "doi-norm",
                "db_title": db_row.get("title"),
                "csv_title": "",
                "score": None,
                "method": None,
                "key": "",
            }

    # -------------------------------------------------------------------
    # DRY-RUN / APPLY report
    # -------------------------------------------------------------------
    print("=" * 70)
    print("RECOVER MISSING DOI — " + ("APPLY MODE" if apply_mode else "DRY-RUN MODE"))
    print("=" * 70)
    print(f"Total candidates (no valid DOI) : {len(candidates)}")
    print(f"  skipped (CJK title)           : {len(skipped_cjk)}")
    print(f"  no/empty title                : {len(no_title)}")
    print(f"  fuzzy matched                 : {len(matched)}")
    print(f"  unmatched                     : {len(unmatched)}")
    print(f"  json files to create          : {sum(1 for m in matched if m['json_to_create'])}")
    print(f"  DOI normalized (total)        : {len(norm_fixed)}  {sorted(norm_fixed)}")
    print(f"  DOI normalized (NEW vs CSV)   : {len(newly_norm)}  {sorted(newly_norm)}")
    total_actions = len(actions)
    print(f"  total PUT actions             : {total_actions}")

    if skipped_cjk:
        sample = skipped_cjk[:10]
        print(f"\nSkipped CJK ids (sample): {sample}" + (" ..." if len(skipped_cjk) > 10 else ""))

    # --- DOI NORMALIZATION detail ---
    print("\n" + "-" * 70)
    print(f"DOI NORMALIZATION ({len(norm_fixed)})")
    print("-" * 70)
    for cid in sorted(norm_fixed):
        tag = "NEW" if cid in newly_norm else "already-covered-by-CSV"
        raw = cand_by_id[cid].get("doi")
        print(f"  id={cid}  {tag}")
        print(f"      raw  : {raw!r}")
        print(f"      norm : {norm_fixed[cid]!r}")

    print("\n" + "-" * 70)
    print(f"MATCHED RECORDS ({len(matched)})")
    print("-" * 70)
    for m in matched:
        norm_note = "  [DOI normalized]" if m["id"] in norm_fixed else ""
        print(f"\nid={m['id']}  score={m['score']:.3f}  method={m['method']}  key={m['key']}{norm_note}")
        print(f"  DB title   : {m['db_title']!r}")
        print(f"  CSV title  : {m['csv_title']!r}")
        print(f"  JSON create: {'YES' if m['json_to_create'] else 'no (exists)'}")
        if m["payload"]:
            print("  PUT payload:")
            for k, v in m["payload"].items():
                shown = v if len(str(v)) <= 200 else str(v)[:200] + " ..."
                print(f"      {k} = {shown!r}")
        else:
            print("  PUT payload: (none — no empty fields to fill)")

    # --- Normalization-only (no fuzzy match) ---
    norm_only = [a for a in actions.values() if a["source"] == "doi-norm"]
    if norm_only:
        print("\n" + "-" * 70)
        print(f"DOI-NORMALIZED ONLY (no fuzzy match) ({len(norm_only)})")
        print("-" * 70)
        for a in norm_only:
            print(f"\nid={a['id']}  (PUT doi only)")
            print(f"  DB title : {a['db_title']!r}")
            print(f"  PUT payload: doi = {a['payload']['doi']!r}")

    print("\n" + "-" * 70)
    print(f"UNMATCHED RECORDS ({len(unmatched)})")
    print("-" * 70)
    for doc_id, title in unmatched:
        print(f"  id={doc_id}  title={title!r}")

    # -------------------------------------------------------------------
    # APPLY: perform the writes
    # -------------------------------------------------------------------
    if apply_mode:
        print("\n" + "=" * 70)
        print("APPLYING CHANGES")
        print("=" * 70)
        ok_api = 0
        fail_api = 0
        json_created = 0
        json_fail = 0
        verified = []
        api_errors = {}

        for a in actions.values():
            doc_id = a["id"]
            payload = a["payload"]
            # 1) PUT to API
            if payload:
                try:
                    r = requests.put(
                        f"{API_BASE}/assist/api/documents/{doc_id}",
                        headers=API_HEADERS,
                        json=payload,
                        timeout=30,
                    )
                    r.raise_for_status()
                    ok_api += 1
                    verified.append(doc_id)
                except Exception as e:
                    fail_api += 1
                    api_errors[doc_id] = str(e)
                    print(f"[ERROR] PUT id={doc_id}: {e}")
            # 2) Write missing markdown JSON (fuzzy-matched rows only)
            if a["matched_row"] is not None and a["json_to_create"]:
                path = os.path.join(MARKDOWNS_DIR, f"{doc_id}.json")
                try:
                    with open(path, "w", encoding="utf-8") as f:
                        json.dump(a["matched_row"], f, ensure_ascii=False, indent=2)
                    json_created += 1
                except Exception as e:
                    json_fail += 1
                    print(f"[ERROR] write json id={doc_id}: {e}")

        # Sanity GET: one full-payload id (7 or 3195) + one DOI-normalized (69).
        sanity_ids = []
        if 69 in verified:
            sanity_ids.append(69)            # DOI-normalized example
        for pref in (7, 3195):
            if pref in verified and pref not in sanity_ids:
                sanity_ids.append(pref)      # full-payload example
        for vid in verified:
            if vid not in sanity_ids:
                sanity_ids.append(vid)
            if len(sanity_ids) >= 3:
                break

        print("\nSanity GET on updated ids:")
        for doc_id in sanity_ids:
            try:
                r = requests.get(
                    f"{API_BASE}/assist/api/documents/{doc_id}",
                    headers={"X-Token": API_TOKEN},
                    timeout=30,
                )
                r.raise_for_status()
                d = r.json()
                print(f"  id={doc_id}: doi={d.get('doi')!r} year={d.get('year')!r} "
                      f"authors={'set' if d.get('authors') else 'empty'} "
                      f"journal={'set' if d.get('journal') else 'empty'} "
                      f"abstract={'set' if d.get('abstract') else 'empty'}")
            except Exception as e:
                print(f"  [ERROR] GET id={doc_id}: {e}")

        print("\n=== Apply Summary ===")
        supplemented = len([a for a in actions.values() if a["payload"]])
        print(f"Records supplemented (PUT) : {supplemented}")
        print(f"  API PUT success          : {ok_api}")
        print(f"  API PUT failed           : {fail_api}")
        print(f"JSON files created         : {json_created}")
        print(f"JSON files failed          : {json_fail}")
        print(f"DOIs normalized            : {len(norm_fixed)}  {sorted(norm_fixed)}")
        print(f"  of which NEW (vs CSV)    : {len(newly_norm)}  {sorted(newly_norm)}")
        if api_errors:
            print("Per-id API errors:")
            for doc_id, err in sorted(api_errors.items()):
                print(f"  id={doc_id}: {err}")
    else:
        print("\n[DRY-RUN] No API calls or file writes were performed.")


if __name__ == "__main__":
    main()
