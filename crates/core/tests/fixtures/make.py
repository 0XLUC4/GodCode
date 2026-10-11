"""Rebuilds the SQLite databases the tests of `crates/core/src/sqlite.rs` read.

Run it from anywhere: `python crates/core/tests/fixtures/make.py`. The tests state what
these files hold, so change both together.
"""

import shutil
import sqlite3
from pathlib import Path

HERE = Path(__file__).parent


def fresh(name: str) -> Path:
    path = HERE / name
    for stale in (path, Path(f"{path}-wal"), Path(f"{path}-shm")):
        stale.unlink(missing_ok=True)
    return path


def letters(length: int) -> str:
    return "".join(chr(ord("a") + index % 26) for index in range(length))


def sample() -> None:
    """Every kind of table and value the reader has to cope with, on 512-byte pages."""
    path = fresh("sample.db")
    db = sqlite3.connect(path)
    db.execute("PRAGMA page_size = 512")

    db.execute(
        """CREATE TABLE people (
            id INTEGER PRIMARY KEY,
            "full name" TEXT NOT NULL, -- a comment, with a comma
            age INT,
            score REAL DEFAULT 0.5,
            photo BLOB,
            note TEXT,
            CONSTRAINT sane CHECK (age IS NULL OR age >= 0),
            UNIQUE ("full name")
        )"""
    )
    db.executemany(
        "INSERT INTO people VALUES (?, ?, ?, ?, ?, ?)",
        [
            (1, "Ada Lovelace", 36, 9.5, bytes([0, 255, 16]), None),
            (2, "Zoë — 日本語 😀", None, -0.25, None, "héllo"),
            (40, "Bob", 0, 1e300, b"", ""),
        ],
    )

    # No declared type: each value keeps the size SQLite picks for it.
    db.execute("CREATE TABLE numbers (label TEXT, value)")
    db.executemany(
        "INSERT INTO numbers VALUES (?, ?)",
        [
            ("zero", 0),
            ("one", 1),
            ("minus one", -1),
            ("one byte", 127),
            ("one byte, negative", -128),
            ("two bytes", 32767),
            ("two bytes, negative", -32768),
            ("three bytes", 8388607),
            ("three bytes, negative", -8388608),
            ("four bytes", 2147483647),
            ("four bytes, negative", -2147483648),
            ("six bytes", 140737488355327),
            ("six bytes, negative", -140737488355328),
            ("eight bytes", 9223372036854775807),
            ("eight bytes, negative", -9223372036854775808),
            ("real", 2.0),
            ("small real", -1.5e-7),
        ],
    )

    db.execute("CREATE TABLE events (id INTEGER PRIMARY KEY, kind TEXT, payload TEXT)")
    db.executemany(
        "INSERT INTO events VALUES (?, ?, ?)",
        [(index * 3, f"kind-{index % 7}", f"event number {index}") for index in range(1, 1501)],
    )
    db.execute("CREATE INDEX numbers_label ON numbers (label)")

    # Rows on both sides of what fits in a page (477 bytes, of which 12 are not the body),
    # then far beyond it.
    db.execute("CREATE TABLE documents (title TEXT, body TEXT, data BLOB)")
    lengths = [*range(463, 469), 534, 535, 536, 1000, 3000]
    db.executemany(
        "INSERT INTO documents VALUES (?, ?, NULL)",
        [(f"len-{length}", letters(length)) for length in lengths],
    )
    db.execute("INSERT INTO documents VALUES ('unicode', ?, ?)", ("é😀 " * 400, bytes(2000)))

    db.execute("CREATE TABLE scratch (junk TEXT)")
    db.executemany("INSERT INTO scratch VALUES (?)", [(letters(200),) for _ in range(10)])

    db.execute("CREATE VIEW adults AS SELECT * FROM people WHERE age >= 18")
    db.execute(
        """CREATE TRIGGER people_note AFTER INSERT ON people
           BEGIN UPDATE people SET note = 'new' WHERE id = NEW.id AND 0; END"""
    )

    db.execute("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT) WITHOUT ROWID")
    db.execute("INSERT INTO settings VALUES ('theme', 'dark')")

    # `gross` is computed when read: the file holds nothing for it.
    db.execute(
        """CREATE TABLE prices (
            item TEXT, net REAL,
            gross REAL GENERATED ALWAYS AS (net * 1.2) VIRTUAL,
            label TEXT
        )"""
    )
    db.execute("INSERT INTO prices VALUES ('tea', 3, 'hot')")

    db.execute("CREATE TABLE tags (tag_id INTEGER, label TEXT, PRIMARY KEY (tag_id))")
    db.executemany("INSERT INTO tags VALUES (?, ?)", [(5, "five"), (9, "nine")])

    db.execute("CREATE TABLE notes (body TEXT)")
    db.execute("INSERT INTO notes VALUES ('written before the column came')")
    db.execute("ALTER TABLE notes ADD COLUMN pinned INTEGER")
    db.execute("INSERT INTO notes VALUES ('written after', 1)")

    db.execute(
        '''CREATE TABLE "odd ""names""" (
            [first col] TEXT, `second` INT, 'third' TEXT,
            "fo,ur" NUMERIC(10, 2) DEFAULT (1 + 2)
        )'''
    )
    db.execute("CREATE TABLE empty (a, b)")

    db.execute("DROP TABLE scratch")
    db.commit()
    assert db.execute("PRAGMA freelist_count").fetchone()[0] > 0, "no free page left behind"
    db.close()


def utf16() -> None:
    path = fresh("utf16.db")
    db = sqlite3.connect(path)
    db.execute("PRAGMA page_size = 512")
    db.execute("PRAGMA encoding = 'UTF-16le'")
    db.execute("CREATE TABLE words (word TEXT)")
    db.execute("INSERT INTO words VALUES ('wide')")
    db.commit()
    db.close()


def wal() -> None:
    """A file copied while its newest rows are still in the log beside it."""
    path = fresh("wal.db")
    live = fresh("wal-live.db")
    db = sqlite3.connect(live)
    db.execute("PRAGMA page_size = 512")
    db.execute("CREATE TABLE early (word TEXT)")
    db.execute("INSERT INTO early VALUES ('checkpointed')")
    db.commit()
    db.execute("PRAGMA journal_mode = WAL")
    db.execute("INSERT INTO early VALUES ('still in the log')")
    db.commit()
    shutil.copyfile(live, path)
    db.close()
    fresh("wal-live.db")


if __name__ == "__main__":
    sample()
    utf16()
    wal()
    for built in sorted(HERE.glob("*.db")):
        print(f"{built.name}: {built.stat().st_size} bytes")
