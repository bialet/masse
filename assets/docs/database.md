# Database reference (Bialet Query API)

Bialet embeds SQLite and exposes it through the backtick Query literal. Raw SQL
syntax (SELECT, JOIN, aggregates, PRAGMA, …) lives in `sqlite.md`; this page
covers the Query object itself and the helpers Bialet adds on top of it.

```wren
`SELECT * FROM users WHERE active = 1`.fetch
```

## Running a query

| Method | Returns |
| --- | --- |
| `.query` | Runs INSERT/UPDATE/DELETE; for INSERT returns the last inserted id |
| `.fetch` | All rows as a List of Maps |
| `.first` | First row as a Map (adds `LIMIT 1`) |
| `.val`   | First column of the first row, as a string (adds `LIMIT 1`) |
| `.toNum` | First column of the first row, as a number (adds `LIMIT 1`) |
| `.toBool`| First column of the first row, as a boolean (adds `LIMIT 1`) |

Each can be called or read as a property: `.fetch`, `.first(id)`, `.val(id)`,
`.toNum`. Never write your own `LIMIT 1` for `first`/`val`/`toNum`/`toBool` —
they add it, so `... LIMIT 1`.first becomes `... LIMIT 1 LIMIT 1` and fails.

## Parameters: three positional max, otherwise an array

Values go through `?` placeholders. You **cannot** concatenate or interpolate
into the SQL string. Every method accepts the same parameter shapes:

- `()` — no parameters
- `(param1)` — one parameter (converted to string)
- `(param1, param2)` — two parameters
- `(param1, param2, param3)` — three parameters
- `([p1, p2, p3, p4, ...])` — an **array** when you need more than three

**This is a hard limit.** Passing four or more positional arguments is a runtime
error, not a silent failure:

```wren
// WRONG — five positional args:
//   Runtime Error: Query does not implement 'fetch(_,_,_,_,_)'.
`SELECT * FROM recipes WHERE (? = '' OR title LIKE '%' || ? || '%')
   AND (? = '' OR category = ?)
   AND owner = ?`.fetch(search, search, category, category, owner)

// RIGHT — wrap the values in an array
`SELECT * FROM recipes WHERE (? = '' OR title LIKE '%' || ? || '%')
   AND (? = '' OR category = ?)
   AND owner = ?`.fetch([search, search, category, category, owner])
```

```wren
// WRONG — interpolation is rejected outright
var id = 1
`SELECT * FROM users WHERE id = %(id)`.first(id)

// RIGHT
var id = 1
`SELECT * FROM users WHERE id = ?`.first(id)

// An array also works for a single value
`SELECT * FROM users WHERE id = ?`.first([id])
```

## Optional filters without concatenation

Because the WHERE clause can't be built up fragment by fragment, write one
static query with a bypass in front of each filter. `OR` short-circuits the
filter away when it doesn't apply:

```wren
var search = Request.get("search") || ""
var users = `SELECT * FROM users WHERE (? = '' OR name LIKE '%' || ? || '%')`.fetch(search, search)

// Boolean bypass — tri-state "all / active / completed"
static list(filter) { `SELECT * FROM tasks
  WHERE session = ? AND (? = 1 OR finished = ?)
  ORDER BY createdAt ASC`.fetch(Session.id, filter == "all", filter == "active").to(Task) }
```

Booleans bind as `1`/`0`. Chain independent filters with `AND`, each with its own
bypass; once there are more than three of them, pass the values as an array.

## Safe sorting with `.order()`

```wren
order(column, direction, allowedColumns)
order(column, direction, allowedColumns, limit)
```

`allowedColumns` is a whitelist; a column outside it falls back to the first
entry, so a user-controlled `?sort=` cannot inject SQL. `direction` is
normalized to `ASC`/`DESC`. Returns a Query, so chain `.fetch`:

```wren
var allowed = ["id", "name", "createdAt"]
var col = Request.get("sort") || "id"
var dir = Request.get("order") || "asc"
var users = `SELECT * FROM users`.order(col, dir, allowed).fetch
```

## Pagination

Always pass `LIMIT`/`OFFSET` as `?` placeholders:

```wren
var limit = Num.fromString(Request.get("limit") || "20")
var offset = (Num.fromString(Request.get("page") || "1") - 1) * limit
var users = `SELECT * FROM users ORDER BY id LIMIT ? OFFSET ?`.fetch(limit, offset)
```

## Insert and update

```wren
// Raw SQL INSERT returns the new id
var id = `INSERT INTO users (name, email) VALUES (?, ?)`.query("John", "john@example.com")

// `table`.save(map) — INSERT when there is no "id" key, UPDATE when there is
var id = `users`.save({"name": "John", "email": "john@example.com"})
`users`.save({"id": id, "name": "John Doe", "email": "john@example.com"})
```

`save()` also accepts a class instance. It persists **one value per underlying
field**, in the order the fields are first referenced in the class body — so
every real column needs a matching field/getter, or `save()` fails with
`table X has N columns but M values were supplied`. A getter that returns null
also defeats a column `DEFAULT` (SQLite only applies defaults to omitted
columns), so set the value yourself when the field is empty.

## Mapping rows to classes with `.to(Class)`

```wren
class User {
  construct new(data) {
    _id = data["id"]
    _name = data["name"] || ""
  }
  id { _id }
  name { _name }
}

var user = `SELECT * FROM users WHERE id = ?`.first(id).to(User)   // instance or null
var users = `SELECT * FROM users`.fetch.to(User)                   // List of instances
```

`.to(Class)` calls `Class.new(row)` for each row, passing the row as a Map keyed
by column name. It works on `.first`, `.fetch`, a single Map, or a List of Maps.

## Migrations

`Db.migrate(name, sql)` runs once per unique `name`, at startup and whenever a
`.wren` file changes. Put it in `_migration.wren` or `_app/migration.wren`;
statements can be separated with `;`.

```wren
Db.migrate("Create users", `
  CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    createdAt DATETIME DEFAULT CURRENT_TIMESTAMP
  )
`)
```

## Gotchas

- **More than three parameter values require the array form** — otherwise you
  get `Query does not implement 'fetch(_,_,_,_,_)'` (or the same for `_`-arity on
  `query`/`first`/`val`/`toNum`/`toBool`).
- Column values come back as **strings**: use `.toNum`, `.toBool`, or
  `Num.fromString(row["age"])` before doing arithmetic.
- Never interpolate values — use `?` and pass parameters.
- `.first`/`.val`/`.toNum`/`.toBool` add their own `LIMIT 1`; do not add another.
- `BIALET_*` tables (`BIALET_CONFIG`, `BIALET_MIGRATIONS`, `BIALET_SESSION`,
  `BIALET_FILES`, …) are managed by Bialet — don't drop or alter them.
