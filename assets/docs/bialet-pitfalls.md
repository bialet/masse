# Bialet / Wren pitfalls (verified)

These are the traps that repeatedly cost entire turns in real app builds. Each
one was reproduced with `bialet -r` or against a running app; the error text is
the one Bialet actually prints.

## 1. `toString` is a getter, not a method

```wren
name=(val) { _name = val.toString.trim }      // correct
// name=(val) { _name = val.toString().trim() } // String does not implement 'toString()'
```

Wren resolves `toString` as a property. Adding `()` turns it into a missing
method. Same for `.trim`, `.count`, `.keys`, `.values`: no parentheses.

## 2. Never compare a Date (or a user class) to `null` with `date == null`

```wren
if (!date) { ... }          // safe
if (null == date) { ... }   // safe
// if (date == null) ...    // Runtime Error: Null does not implement 'unix'.
```

`date == null` calls `Date.==`, which reads `other.unix`; `null` has no `unix`.
`!date` is `true` only when the value is null (a Date makes `!date` return null,
which is falsy), so it is the idiomatic guard. The same rule applies to your own
classes: put the null check on the left, or test `if (!value)`.

## 3. Every method that produces a value needs an explicit `return`

Wren is not expression-oriented: a method returns the value of `return`, not the
last expression.

```wren
static forDay(day) {
  return `SELECT * FROM orders WHERE deliveryDate = ?`.fetch(day).to(Order)
}
```

A missing `return` yields `null`; the caller then does `for (row in null)` and
**the whole Bialet process crashes with SIGSEGV (exit 139)**. The browser sees
`000` / an empty reply, not a nice error.

## 4. Never iterate a value that may be null

```wren
var rows = `SELECT ...`.fetch
if (!rows) rows = []
for (row in rows) { ... }
```

`for (x in null)` is a hard crash, not an exception. Guard every Query result
and every helper that can return null.

## 5. A literal `%` in a Wren string is `\x25`

```wren
System.print("100\x25")   // 100%
// System.print("100%")   // Compilation Error: Expect '(' after '%'.
```

`%(` starts interpolation, so a bare `%` is a syntax error. In an **HTML
template** `%` is plain text: write `100%` there (if the template parser
complains, use the entity `&#37;`). Do not write `\x25` in HTML: it renders
literally as `\x25`.

## 6. `BIALET_CONFIG` stores `(key, val)`, not `(key, value)`

```wren
Config.set("fee_amount", "20000")   // correct
// INSERT INTO BIALET_CONFIG (key, value) ...
// Query Error: table BIALET_CONFIG has no column named value
```

Use `Config.set/get/bool/num/enable/disable/json`; never hand-write the table.

## 7. Query methods take at most three positional arguments

```wren
`SELECT * FROM x WHERE a=? AND b=? AND c=? AND d=?`.fetch([a, b, c, d])
// .fetch(a, b, c, d) -> Query does not implement 'fetch(_,_,_,_)'
```

The same limit applies to `.first`, `.val`, `.toNum`, `.toBool`, `.query` and
`.order`. Four or more values go in one List.

## 8. Every file imports what it uses

```wren
// route file
import "/_app/template" for Layout
import "/_app/domain" for Item, Member

// _app/template.wren that calls Periods
import "/_app/domain" for Periods
```

A template is a separate module: it does not inherit the route's imports. This
is why `{{ Periods.label(...) }}` can fail with `Variable is used but not
defined` even though `Periods` exists in `_app/domain.wren`.

## 9. `_tests/_init.wren` variables are global to every test file

```wren
// _tests/_init.wren
var day = "2026-05-04"

// _tests/flujo.wren
// var day = "2026-05-04"   // Module variable is already defined: day
```

Use the variable from `_init.wren`, give the local one another name, or wrap the
shared data in a function. `Test` is also preloaded: never
`import "bialet/test" for Test`.

## 10. Tests use a temporary database; manual HTTP does not

`bialet -T` runs against a temp DB, so test rows do not pollute `_db.sqlite3`.
Manual `curl` POSTs against `bialet -p` **do** write to `_db.sqlite3`: when you
verify by hand, delete the rows you inserted (or back up and restore the file).

## 11. `Date.new(unixSeconds)` does not exist

`Date.new(1792292400)` is a year-1792292400 date, not a timestamp. For date
arithmetic use the verified `Dates` helpers in `bialet-recipes.md`; for "now"
use `Date.new()`.

## 12. `Num.fromString` does not understand `1.234,50`

It returns null for thousands separators and decimal commas. Normalize first
(see `Money.parse` in `bialet-recipes.md`), or store integer cents and parse the
form value with the helper.

## 13. Assertions match raw markup

`Test.get("/").contains("device")` also matches `width=device-width` in the
viewport meta tag. Assert on visible, unique text (a heading, a row label), not
on a substring that appears in the HTML boilerplate.

## 14. `bialet_run` is not a request

`bialet_run` has no `BIALET_*` tables, no session and no router. `Session.new()`,
`Config.get(...)` and app queries fail there. Use `_tests/` + `bialet_test`, or
start the server and use `curl`, for framework behaviour.
