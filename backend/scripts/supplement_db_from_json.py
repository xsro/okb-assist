#!/usr/bin/env python3
"""Supplement empty columns of the `documents` table from Zotero-style JSON files.

For each <id>.json in MARKDOWNS_DIR, fill only the EMPTY/NULL columns of the
matching DB row (documents.id == <id>) with values taken from the JSON.
Existing DB data is never overwritten.

Usage:
    python3 supplement_db_from_json.py            # execute and commit
    python3 supplement_db_from_json.py --dry-run  # preview, no writes
"""

import argparse
import glob
import json
import os
import sqlite3

DB_PATH = "/home/orangepi/sys/okb-assist/okb_assist.db"
MARKDOWNS_DIR = "/home/orangepi/sys/okb-knowledge/markdowns"

# Whether the DB `authors` column stores a JSON-array string (["A","B"]) as
# opposed to a plain "Last, First; ..." string. Auto-detected in detect_flags().
DB_AUTHORS_IS_JSON_ARRAY = True

# JSON key -> DB column
FIELD_MAP = [
    ("Title", "title"),
    ("Author", "authors"),
    ("Publication Year", "year"),
    ("DOI", "doi"),
    ("Publication Title", "journal"),
    ("Abstract Note", "abstract"),
    ("Language", "language"),
]


def detect_flags(db_path):
    """Detect how `authors` is stored in the DB, by sampling non-empty values."""
    con = sqlite3.connect(db_path)
    try:
        cur = con.cursor()
        rows = cur.execute(
            "SELECT authors FROM documents "
            "WHERE authors IS NOT NULL AND authors != '' LIMIT 20"
        ).fetchall()
    finally:
        con.close()

    nonempty = [r[0] for r in rows if r[0]]
    if not nonempty:
        # No existing authors data; default assumption is JSON-array form.
        return True
    return nonempty[0].lstrip().startswith("[")


def is_empty(value):
    """True if the DB value is NULL or empty/whitespace-only."""
    if value is None:
        return True
    if isinstance(value, str) and value.strip() == "":
        return True
    return False


def to_year(json_obj):
    """Extract an int year from 'Publication Year' or 'Date'."""
    py = json_obj.get("Publication Year")
    if py and str(py).strip():
        try:
            return int(str(py).strip())
        except ValueError:
            pass
    date = json_obj.get("Date")
    if date and str(date).strip():
        s = str(date).strip()
        for token in s.split("-"):
            if len(token) == 4 and token.isdigit():
                try:
                    return int(token)
                except ValueError:
                    pass
    return None


def convert_authors(zotero_author):
    """Convert Zotero 'Author' ("Last, First; Last2, First2") into a
    JSON-array string of 'First Last' form."""
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


def build_update(doc_id, json_obj, row, dry_run):
    """Given a DB row dict and a parsed JSON object, return (col_values, updates)
    where `updates` is a list of (column, value) tuples for columns that are
    currently empty and have a non-empty JSON source."""
    updates = []
    for json_key, db_col in FIELD_MAP:
        cur_val = row[db_col]

        # Source value from JSON
        if db_col == "year":
            src = to_year(json_obj)
        elif db_col == "authors":
            if DB_AUTHORS_IS_JSON_ARRAY:
                src = convert_authors(json_obj.get("Author"))
            else:
                raw = json_obj.get("Author")
                src = raw.strip() if isinstance(raw, str) else (raw if raw else None)
        else:
            raw = json_obj.get(json_key)
            src = raw.strip() if isinstance(raw, str) else raw

        if is_empty(cur_val) and src not in (None, "") and str(src).strip() != "":
            if db_col == "year":
                updates.append((db_col, int(src)))
            else:
                updates.append((db_col, src))

    if not updates:
        return None

    set_clause = ", ".join(f"{col} = ?" for col, _ in updates)
    sql = f"UPDATE documents SET {set_clause} WHERE id = ?"
    params = [val for _, val in updates] + [doc_id]

    if dry_run:
        print(f"[dry-run] id={doc_id}")
        for col, val in updates:
            print(f"    SET {col} = {val!r}")
    return (sql, params, updates)


def main():
    global DB_AUTHORS_IS_JSON_ARRAY

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Preview the updates without executing or committing.",
    )
    args = parser.parse_args()

    DB_AUTHORS_IS_JSON_ARRAY = detect_flags(DB_PATH)
    print(f"DB_AUTHORS_IS_JSON_ARRAY = {DB_AUTHORS_IS_JSON_ARRAY}")

    con = sqlite3.connect(DB_PATH)
    con.row_factory = sqlite3.Row
    cur = con.cursor()

    files = sorted(glob.glob(os.path.join(MARKDOWNS_DIR, "*.json")))

    scanned = 0
    rows_filled = 0
    per_col = {db_col: 0 for _, db_col in FIELD_MAP}

    for path in files:
        base = os.path.basename(path)
        doc_id = base[:-5]  # strip ".json"
        if not doc_id.isdigit():
            continue
        doc_id = int(doc_id)
        scanned += 1

        try:
            with open(path, encoding="utf-8") as f:
                json_obj = json.load(f)
        except Exception as e:
            print(f"[warn] id={doc_id}: failed to parse JSON ({e})")
            continue

        row = cur.execute("SELECT * FROM documents WHERE id = ?", (doc_id,)).fetchone()
        if row is None:
            continue

        result = build_update(doc_id, json_obj, row, args.dry_run)
        if result is None:
            continue

        sql, params, updates = result
        if not args.dry_run:
            cur.execute(sql, params)
        rows_filled += 1
        for col, _ in updates:
            per_col[col] += 1

    if not args.dry_run:
        con.commit()

    con.close()

    print("\n=== Summary ===")
    print(f"JSON files scanned      : {scanned}")
    print(f"DB rows filled (>=1 col): {rows_filled}")
    print("Per-column fields filled:")
    for col, cnt in per_col.items():
        print(f"    {col:16s}: {cnt}")


if __name__ == "__main__":
    main()
