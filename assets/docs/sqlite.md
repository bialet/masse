# SQLite reference

SQLite is the database Bialet embeds. Use it through the backtick Query object
(`\`SELECT ...\``), never through a quoted string. This page is a compact SQL
reference; for the Bialet Query API (`fetch`, `first`, `val`, `save`, `order`)
see `database.md`.

## Types (affinity)

SQLite has five storage classes. A column's declared type picks one of four
affinities:

| Declared type contains | Affinity | Stores |
| --- | --- | --- |
| `INT` | INTEGER | 1, 2, 3, 4, 6, 8-byte signed ints |
| `CHAR`, `CLOB`, `TEXT` | TEXT | UTF-8/UTF-16 string |
| `BLOB` or none | BLOB | raw bytes |
| `REAL`, `FLOA`, `DOUB` | REAL | 8-byte IEEE float |
| `NUMERIC`, `DECIMAL`, `BOOLEAN`, `DATE`, `DATETIME` | NUMERIC | int or real |

SQLite is dynamically typed: a TEXT column can hold an integer. Declare types
for clarity and for `INTEGER PRIMARY KEY` (the rowid alias), but expect Bialet
to hand every value back as a string — convert with `Num.fromString`.

## CREATE TABLE

```sql
CREATE TABLE IF NOT EXISTS users (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  name      TEXT NOT NULL,
  email     TEXT UNIQUE,
  active    INTEGER NOT NULL DEFAULT 1,
  createdAt DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

- `INTEGER PRIMARY KEY` is an alias for the internal `rowid` (fast lookups).
- `AUTOINCREMENT` prevents rowid reuse after deletes (rarely needed).
- `NOT NULL`, `UNIQUE`, `CHECK (expr)`, `DEFAULT` are enforced.
- `STRICT` at the end enforces real types (SQLite 3.37+):
  `CREATE TABLE t(a INTEGER) STRICT;`

## SELECT

```sql
SELECT id, name FROM users WHERE active = 1 ORDER BY name COLLATE NOCASE;
SELECT COUNT(*) AS n FROM users;
SELECT DISTINCT city FROM users;
SELECT * FROM users LIMIT 20 OFFSET 40;
SELECT name, COUNT(*) FROM users GROUP BY name HAVING COUNT(*) > 1;
SELECT u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id;
SELECT * FROM users WHERE name LIKE 'a%';        -- case-insensitive for ASCII
```

Aggregates: `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, `GROUP_CONCAT`, `TOTAL`.
`ORDER BY ... ASC|DESC`, `NULLS FIRST|LAST` (3.30+).

## INSERT / UPDATE / DELETE

```sql
INSERT INTO users (name, email) VALUES ('Ana', 'ana@example.com');
INSERT INTO users (name) VALUES ('A'), ('B'), ('C');   -- multi-row
UPDATE users SET active = 0 WHERE id = 5;
DELETE FROM users WHERE createdAt < '2024-01-01';
```

Upsert (3.24+):

```sql
INSERT INTO users (id, name) VALUES (1, 'Ana')
ON CONFLICT(id) DO UPDATE SET name = excluded.name;
```

`excluded` is the row that would have been inserted. `INSERT OR IGNORE` skips
rows that violate a constraint; `RETURNING id, name` (3.35+) gives back rows
from INSERT/UPDATE/DELETE.

## Joins and subqueries

```sql
SELECT * FROM a LEFT JOIN b ON b.a_id = a.id;   -- INNER, LEFT, CROSS
SELECT name FROM users WHERE id IN (SELECT user_id FROM orders);
SELECT name, (SELECT COUNT(*) FROM orders o WHERE o.user_id = u.id) AS n FROM users u;
WITH recent AS (SELECT * FROM posts ORDER BY createdAt DESC LIMIT 10)
SELECT * FROM recent WHERE published = 1;
```

## Transactions

```sql
BEGIN;
UPDATE accounts SET balance = balance - 100 WHERE id = 1;
UPDATE accounts SET balance = balance + 100 WHERE id = 2;
COMMIT;            -- or ROLLBACK;
```

Implicit transactions wrap every statement; group writes explicitly when they
must be atomic. Bialet's `Db.migrate` runs its `;`-separated statements in a
transaction.

## Dates and time

```sql
SELECT DATE('now');                              -- 2026-10-13
SELECT DATETIME('now');                          -- 2026-10-13 12:00:00
SELECT strftime('%Y', 'now');                    -- 2026
SELECT strftime('%Y-%m', 'now');                 -- 2026-10
SELECT DATE('now', '-7 days');
SELECT strftime('%s', 'now');                    -- unix seconds
SELECT DATETIME('now', 'unixepoch');             -- from unix seconds
```

Modifiers: `+N seconds|minutes|hours|days|months|years`, `start of month`,
`weekday N`, `unixepoch`, `localtime`, `utc`.

## PRAGMA

```sql
PRAGMA table_info(users);        -- columns, types, defaults
PRAGMA foreign_keys = ON;        -- OFF by default: enable per connection
PRAGMA journal_mode = WAL;       -- concurrent readers + one writer
PRAGMA index_list(users);
PRAGMA user_version;             -- integer you control for schema versioning
```

## Indexes and keys

```sql
CREATE INDEX idx_users_email ON users(email);
CREATE UNIQUE INDEX idx_users_name ON users(name COLLATE NOCASE);

CREATE TABLE orders (
  id      INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE
);
```

Foreign keys are only enforced when `PRAGMA foreign_keys = ON` is set on the
connection. Index the columns you filter, join or sort on.

## ALTER TABLE

```sql
ALTER TABLE users ADD COLUMN phone TEXT;
ALTER TABLE users RENAME TO people;
ALTER TABLE users RENAME COLUMN email TO mail;
ALTER TABLE users DROP COLUMN phone;            -- 3.35+
```

Dropping/altering constraints needs the 12-step table rebuild (create new,
copy, drop old, rename).

## Full-text search (FTS5)

```sql
CREATE VIRTUAL TABLE docs USING fts5(title, body);
INSERT INTO docs VALUES ('Hello', 'the quick brown fox');
SELECT * FROM docs WHERE docs MATCH 'quick';
```

## JSON (3.38+)

```sql
SELECT json_extract('{"a":1}', '$.a');           -- 1
SELECT json_extract(col, '$.name') FROM t;
SELECT value FROM t, json_each(t.tags);          -- unnest a JSON array
```

## Gotchas

- No boolean type: store 0/1. `TRUE`/`FALSE` are aliases since 3.23.
- `NULL` comparisons yield NULL, not false: use `IS NULL` / `IS NOT NULL`.
- Every value crosses into Bialet as text: always `Num.fromString`.
- `.first`/`.val`/`.toNum` add their own `LIMIT 1`; do not write another.
- Prepared statements use `?` placeholders — never string-concatenate SQL.
