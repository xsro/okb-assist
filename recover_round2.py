#!/usr/bin/env python3
"""
okb-assist data-recovery ROUND 2.
For records lacking a valid DOI and with a non-CJK (English) title that were NOT
found in infos.csv, look up DOI + metadata from Crossref by title similarity,
then push recovered fields via the API (supplement-only, partial update).

Run:  python recover_round2.py --dryrun     (print plan, no writes)
      python recover_round2.py --apply       (PUT accepted payloads)
"""
import argparse
import html
import json
import re
import sqlite3
import time
import sys

import requests

DB_PATH = "/home/orangepi/sys/okb-assist/okb_assist.db"
API_BASE = "http://localhost:5001/assist/api/documents"
TOKEN = "sysusz422"
CROSSREF = "https://api.crossref.org/works"
MAILTO = "okb-recover@example.com"
UA = "okb-assist/1.0 (mailto:okb-recover@example.com)"
CJK_RE = re.compile(r"[\u3000-\u9fff\uff00-\uffef\u3040-\u30ff\uac00-\ud7af]")
DOI_RE = re.compile(r"^10\.\d{4,9}/.+$")
THRESHOLD = 0.85

# fields selected from DB and used for "empty?" checks
SELECT_FIELDS = [
    "id", "title", "authors", "year", "doi", "journal", "abstract",
    "language", "doc_type", "keywords", "title_en", "authors_en",
    "abstract_en", "journal_en",
]


def is_valid_doi(v):
    return bool(v) and bool(DOI_RE.match(v.strip()))


def strip_markup(t):
    """Remove Crossref/HTML markup but KEEP the text inside math tags
    (e.g. <tex-math>$Q$</tex-math> -> Q) so similarity isn't destroyed."""
    if not t:
        return ""
    # extract tex-math / math content, keep the inner expression
    t = re.sub(r"<tex-math[^>]*>(.*?)</tex-math>", r" \1 ", t, flags=re.S | re.I)
    t = re.sub(r"<mml:math[^>]*>(.*?)</mml:math>", r" \1 ", t, flags=re.S | re.I)
    t = re.sub(r"<[^>]+>", "", t)          # drop remaining tags
    t = re.sub(r"\$([^$]*)\$", r"\1", t)   # drop LaTeX $ delimiters, keep body
    t = t.replace("$_{\\infty}$", "∞").replace("$_{∞}$", "∞")
    return t


def clean_title(t):
    if not t:
        return ""
    # decode entities, then strip HTML/markup but keep math text
    t = strip_markup(html.unescape(t))
    t = re.sub(r"\s+", " ", t).strip()
    return t


def normalize(t):
    if not t:
        return ""
    t = t.lower()
    t = re.sub(r"[^a-z0-9\s]", " ", t)
    t = re.sub(r"\s+", " ", t).strip()
    return t


def doc_type_map(t):
    return {
        "journal-article": "article",
        "proceedings-article": "conference",
        "proceedings": "conference",
        "book": "book",
        "book-chapter": "book_chapter",
        "posted": "preprint",
    }.get(t, t)


def crossref_authors(item):
    out = []
    for a in item.get("author", []) or []:
        given = (a.get("given") or "").strip()
        family = (a.get("family") or "").strip()
        if given and family:
            out.append(f"{given} {family}")
        elif family:
            out.append(family)
        elif given:
            out.append(given)
    return out


def get_candidates(conn):
    cur = conn.cursor()
    # records lacking a valid DOI (mirrors the documented predicate)
    cur.execute(
        """
        SELECT id, title, authors, year, doi, journal, abstract, language,
               doc_type, keywords, title_en, authors_en, abstract_en, journal_en
        FROM documents
        WHERE doi IS NULL OR doi='' OR (doi NOT LIKE '10.%' OR doi NOT LIKE '%/%')
        """
    )
    rows = cur.fetchall()
    cands = []
    for r in rows:
        rec = dict(zip(SELECT_FIELDS, r))
        title = rec.get("title")
        if not title or not title.strip():
            continue
        if CJK_RE.search(title):
            continue
        cands.append(rec)
    return cands


def connectivity_ok():
    try:
        r = requests.get(
            f"{CROSSREF}?query.bibliographic=test&rows=1",
            headers={"User-Agent": UA}, timeout=20,
        )
        return r.status_code == 200
    except Exception as e:
        print(f"[connectivity] FAILED: {e}", file=sys.stderr)
        return False


def best_match(cleaned_title, items):
    norm_db = normalize(cleaned_title)
    best = None
    best_score = 0.0
    for it in items:
        ct = it.get("title")
        if not ct:
            continue
        ct0 = ct[0] if isinstance(ct, list) else ct
        if not ct0:
            continue
        # Crossref returns entity-encoded markup (&lt;tex-math&gt;...);
        # decode it, then strip markup but KEEP math text, so a legit match
        # that encodes part of the title in <tex-math> isn't displaced by a
        # noisier (but literally closer) wrong paper
        ct0_clean = strip_markup(html.unescape(ct0))
        score = __import__("difflib").SequenceMatcher(
            None, norm_db, normalize(ct0_clean)
        ).ratio()
        if score > best_score:
            best_score = score
            best = (it, ct0, score)
    return best


def build_payload(rec, matched):
    item, cross_title, score = matched
    doi = item.get("DOI")
    if not is_valid_doi(doi):
        return None, "no valid DOI in crossref item"

    payload = {}
    # doi: always set when DB missing/invalid and crossref valid
    payload["doi"] = doi

    authors = crossref_authors(item)
    authors_str = json.dumps(authors, ensure_ascii=False) if authors else ""
    # authors
    if authors_str and not rec.get("authors"):
        payload["authors"] = authors_str
    # year
    yr = None
    try:
        dp = item.get("issued", {}).get("date-parts")
        if dp and dp[0] and dp[0][0]:
            yr = int(dp[0][0])
    except Exception:
        yr = None
    if yr and rec.get("year") in (None, 0, ""):
        payload["year"] = yr
    # journal
    j = (item.get("container-title") or [None])[0]
    if j and not rec.get("journal"):
        payload["journal"] = j
    # doc_type
    dt = doc_type_map(item.get("type"))
    if dt and not rec.get("doc_type"):
        payload["doc_type"] = dt
    # abstract (often empty from free API — fine)
    ab = item.get("abstract")
    if ab and not rec.get("abstract"):
        # strip jats tags if present
        ab_clean = re.sub(r"<[^>]+>", "", ab)
        payload["abstract"] = ab_clean

    # English docs: also set _en fields when DB empty
    lang = (rec.get("language") or "").lower()
    is_english = (lang in ("", "en", "en-us", "english", "eng"))
    if is_english:
        if cross_title and not rec.get("title_en"):
            payload["title_en"] = strip_markup(html.unescape(cross_title))
        if authors_str and not rec.get("authors_en"):
            payload["authors_en"] = authors_str
        if j and not rec.get("journal_en"):
            payload["journal_en"] = j

    return payload, None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dryrun", action="store_true")
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()
    if not (args.dryrun or args.apply):
        print("Specify --dryrun or --apply")
        sys.exit(2)
    DRY = args.dryrun

    print("=" * 70)
    print(f"MODE: {'DRY-RUN (no writes)' if DRY else 'APPLY (PUT to API)'}")
    print("=" * 70)

    # 1. connectivity
    print("[1] connectivity check ->", end=" ", flush=True)
    if not connectivity_ok():
        print("UNREACHABLE")
        print("STOP: external network unavailable. Do not modify anything.")
        sys.exit(3)
    print("OK")

    # 2. candidates
    conn = sqlite3.connect(DB_PATH)
    cands = get_candidates(conn)
    conn.close()
    print(f"[2] candidates (no valid DOI + non-CJK title): {len(cands)}")

    results = []  # (rec, cross_title, score, doi, payload_or_None, reason)
    for rec in cands:
        cid = rec["id"]
        cleaned = clean_title(rec["title"])
        try:
            r = requests.get(
                CROSSREF,
                params={"query.bibliographic": cleaned, "rows": 5, "mailto": MAILTO},
                headers={"User-Agent": UA},
                timeout=30,
            )
            items = r.json().get("message", {}).get("items", []) if r.ok else []
        except Exception as e:
            results.append((rec, None, 0.0, None, None, f"request error: {e}"))
            time.sleep(1)
            continue

        m = best_match(cleaned, items)
        if not m:
            results.append((rec, None, 0.0, None, None, "no crossref items"))
            time.sleep(1)
            continue
        item, cross_title, score = m
        if score < THRESHOLD or not is_valid_doi(item.get("DOI")):
            results.append(
                (rec, cross_title, score, item.get("DOI"),
                 None, f"low score ({score:.3f}) / no valid DOI")
            )
            time.sleep(1)
            continue

        payload, err = build_payload(rec, m)
        if err:
            results.append((rec, cross_title, score, item.get("DOI"), None, err))
        else:
            results.append((rec, cross_title, score, item.get("DOI"), payload, "ACCEPTED"))
        time.sleep(1)

    # 4. dry-run table
    print("\n[4] DRY-RUN TABLE (all candidates)")
    print("-" * 70)
    header = f"{'id':>6} | {'score':>6} | {'doi to set':<22} | db title  ->  crossref title"
    print(header)
    for rec, cross_title, score, doi, payload, reason in results:
        db_t = (rec["title"] or "")[:45]
        cr_t = (cross_title or "")[:45]
        doi_s = (doi or "")[:22]
        sc = f"{score:.3f}" if score else "-"
        print(f"{rec['id']:>6} | {sc:>6} | {doi_s:<22} | {db_t} -> {cr_t}")
        if payload:
            print(f"       payload fields: {', '.join(payload.keys())}")

    # apply
    applied = 0
    total_fields = 0
    errors = []
    skipped = []
    if not DRY:
        print("\n[4b] APPLY")
        s = requests.Session()
        for rec, cross_title, score, doi, payload, reason in results:
            if not payload:
                skipped.append((rec["id"], reason))
                continue
            cid = rec["id"]
            try:
                r = s.put(
                    f"{API_BASE}/{cid}",
                    headers={"X-Token": TOKEN, "Content-Type": "application/json"},
                    json=payload,
                    timeout=30,
                )
                r.raise_for_status()
                applied += 1
                total_fields += len(payload)
            except Exception as e:
                errors.append((cid, str(e)))

    print("\n[5] SUMMARY")
    print(f"  connectivity: OK")
    print(f"  candidates: {len(cands)}")
    accepted = [x for x in results if x[4]]
    print(f"  got DOI via crossref: {len(accepted)}")
    if not DRY:
        print(f"  applied payloads: {applied}")
        print(f"  total fields supplemented: {total_fields}")
        print(f"  per-id API errors: {errors if errors else 'none'}")
    else:
        print(f"  (dry-run) would apply {len(accepted)} payloads")
    no_doi = [(rec['id'], reason) for rec, _, _, _, payload, reason in results if not payload]
    print(f"  still no DOI ({len(no_doi)}):")
    for cid, why in no_doi:
        print(f"     - id {cid}: {why}")

    # sanity GET on 2 updated ids
    if not DRY and applied:
        sample = [rec['id'] for rec, _, _, _, payload, _ in results if payload][:2]
        print("\n[sanity] GET updated ids:", sample)
        for cid in sample:
            try:
                r = requests.get(f"{API_BASE}/{cid}", headers={"X-Token": TOKEN}, timeout=20)
                if r.ok:
                    d = r.json()
                    print(f"   id {cid}: doi={d.get('doi')!r} journal_en={d.get('journal_en')!r} title_en set={bool(d.get('title_en'))}")
                else:
                    print(f"   id {cid}: GET {r.status_code}")
            except Exception as e:
                print(f"   id {cid}: GET error {e}")


if __name__ == "__main__":
    main()
