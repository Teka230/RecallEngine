#!/usr/bin/env python3
"""Optional maintainer helper to build a local golden corpus.

Never commit real ChatGPT exports or databases. Pass a local source path via
RECALL_GOLDEN_SOURCE; the destination stays outside git by default.
"""

from __future__ import annotations

import os
import sqlite3
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_DB = os.environ.get("RECALL_GOLDEN_SOURCE")
DEST_DB = Path(
    os.environ.get(
        "RECALL_GOLDEN_DEST",
        str(ROOT / "tests" / "fixtures" / "reference-corpus" / "golden.sqlite"),
    )
)
SCHEMA_SQL = ROOT / "src" / "storage" / "migrations" / "001_initial.sql"


def create_golden_corpus() -> None:
    if not SOURCE_DB:
        print(
            "Set RECALL_GOLDEN_SOURCE to a local SQLite path. "
            "Do not commit the output database.",
            file=sys.stderr,
        )
        sys.exit(1)

    source = Path(SOURCE_DB)
    if not source.is_file():
        print(f"Source DB not found at {source}", file=sys.stderr)
        sys.exit(1)

    DEST_DB.parent.mkdir(parents=True, exist_ok=True)
    if DEST_DB.exists():
        DEST_DB.unlink()

    print(f"Opening {source}...")
    src = sqlite3.connect(source)
    dst = sqlite3.connect(DEST_DB)

    print("Copying schema...")
    dst.executescript(SCHEMA_SQL.read_text())

    print("Selecting representative conversations...")
    selected_conv_ids: set[str] = set()

    res = src.execute(
        "SELECT conversation_id FROM messages "
        "GROUP BY conversation_id HAVING count(*) BETWEEN 3 AND 5 LIMIT 1"
    ).fetchone()
    if res:
        selected_conv_ids.add(res[0])

    res = src.execute(
        "SELECT conversation_id FROM messages WHERE role='tool' LIMIT 1"
    ).fetchone()
    if res:
        selected_conv_ids.add(res[0])

    res = src.execute(
        "SELECT conversation_id FROM messages "
        "GROUP BY conversation_id HAVING count(*) > 50 LIMIT 1"
    ).fetchone()
    if res:
        selected_conv_ids.add(res[0])

    print(f"Selected {len(selected_conv_ids)} conversations")
    if not selected_conv_ids:
        print("No conversations selected", file=sys.stderr)
        sys.exit(1)

    conv_placeholders = ",".join(["?"] * len(selected_conv_ids))
    conv_ids_list = list(selected_conv_ids)

    tables_to_filter = {
        "conversations": ("id", conv_ids_list),
        "messages": ("conversation_id", conv_ids_list),
        "nodes": ("conversation_id", conv_ids_list),
    }

    for table, (col, params) in tables_to_filter.items():
        print(f"Copying {table}...")
        rows = src.execute(
            f"SELECT * FROM {table} WHERE {col} IN ({conv_placeholders})", params
        ).fetchall()
        if rows:
            cols = [desc[0] for desc in src.execute(f"SELECT * FROM {table} LIMIT 1").description]
            placeholders = ",".join(["?"] * len(cols))
            dst.executemany(f"INSERT OR IGNORE INTO {table} VALUES ({placeholders})", rows)
            dst.commit()

    print("Copying content_blocks...")
    msg_ids = [r[0] for r in dst.execute("SELECT id FROM messages").fetchall()]
    if msg_ids:
        msg_placeholders = ",".join(["?"] * len(msg_ids))
        rows = src.execute(
            f"SELECT * FROM content_blocks WHERE message_id IN ({msg_placeholders})",
            msg_ids,
        ).fetchall()
        if rows:
            cols = [
                desc[0]
                for desc in src.execute("SELECT * FROM content_blocks LIMIT 1").description
            ]
            placeholders = ",".join(["?"] * len(cols))
            dst.executemany(f"INSERT OR IGNORE INTO content_blocks VALUES ({placeholders})", rows)
            dst.commit()

    print("Copying import_runs...")
    rows = src.execute("SELECT * FROM import_runs").fetchall()
    if rows:
        cols = [desc[0] for desc in src.execute("SELECT * FROM import_runs LIMIT 1").description]
        placeholders = ",".join(["?"] * len(cols))
        dst.executemany(f"INSERT OR IGNORE INTO import_runs VALUES ({placeholders})", rows)
        dst.commit()

    dst.close()
    src.close()
    print(f"Wrote {DEST_DB} — keep it local; do not commit real conversation data.")


if __name__ == "__main__":
    create_golden_corpus()
