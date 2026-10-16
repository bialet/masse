# Testing reference

Bialet ships a built-in tester for HTTP endpoints. Tests are plain Wren files in
the `_tests/` folder and verify your app by calling its routes internally (no
network, no port), against a **fresh temporary database**.

```wren
// _tests/recipes.wren
Test.get("/").status(200).contains("Mis recetas")
Test.post("/recipe", {"title": "Tarta"}).status(302)
```

## Run tests

```bash
bialet -T        # run every file in _tests/
bialet -T -q     # quiet: one summary line + a FAIL block per failure (CI)
```

Exit code is `0` when all tests pass and `1` when any fails. `bialet_test` (the
harness tool) runs the same suite.

## No import — `Test` is already defined

The test runner preloads `Test` into every test file. **Do not import it.** An
`import "bialet/test" for Test` fails with:

```
Error at 'Test': Module variable is already defined.
```

Just start writing assertions at the top of the file.

## Layout

```
myapp/
├── _app.wren
├── _migration.wren
├── hi.wren
└── _tests/          # leading _ keeps the folder private (never served)
    ├── _init.wren   # optional: runs once before the test files
    └── hi.wren      # tests for hi.wren
```

A single test file may hold many independent tests; write one statement per
check. `_tests/_init.wren`, when present, runs once at the start of the run —
use it to seed shared rows.

## HTTP methods

| Call | Sends |
| --- | --- |
| `Test.get(route)` | GET |
| `Test.post(route, params)` | POST with form data (a Map) |
| `Test.apiGet(route)` | GET with a JSON `Content-Type` |
| `Test.apiPost(route, params)` | POST with a JSON body |
| `Test.apiPut(route, params)` | PUT with a JSON body |
| `Test.apiDelete(route)` | DELETE |

Chain `.setHeader(name, value)`, `.method("GET")` or `.postData(map)` to shape
the request. Requests are **lazy**: the call fires when the first assertion runs.

## Assertions

| Assertion | Checks |
| --- | --- |
| `.status(code)` | HTTP status code |
| `.contains(str)` | body contains substring |
| `.notContains(str)` | body does **not** contain substring |
| `.equals(str)` | body equals exactly |
| `.header(name)` / `.header(name, value)` | header present / equals |
| `.json()` | parses the body and returns it (Map or List) |
| `.jsonContains(key)` / `.jsonNotContains(key)` | JSON has / lacks a key |
| `.jsonEquals(key, value)` | JSON key equals value |
| `.assert(condition, message)` | custom assertion |

```wren
Test.get("/page").status(200).contains("Welcome").notContains("Error")

var data = Test.apiGet("/users").status(200).json()
Test.assert(data.count > 0, "expected at least one user")
```

## The acceptance check: POST, then read it back

A form change is only verified when the value goes *in* and comes back *out*.
Assert the round-trip, not just that the page renders. Tests are **flat
statements** — there is no `describe`/`it`/`expect`/`toBe` grouping API:

```wren
// _tests/recipes.wren  (statements run top to bottom, one file = one test)
Test.post("/recipe", {
  "title": "Tarta de manzana",
  "ingredients": "3 manzanas"
}).status(302)

// Read it back on the list and the detail page
Test.get("/").contains("Tarta de manzana")
var id = `SELECT id FROM recipes WHERE title = ?`.val("Tarta de manzana")
Test.get("/recipe?id=%(id)").contains("3 manzanas")
```

You can query the temp database directly from a test to build ids and fixtures —
the tester runs against the same database the requests hit.

## Skipping and the test database

- `Tests.skip()` as the first statement marks a file skipped; it counts
  separately and never affects the exit code.
- Each run creates a temp database, applies your migrations, runs
  `_tests/_init.wren`, then the test files, then deletes it. Your development
  and production data are never touched.

## Gotchas

- **Never import `Test`** — see "No import" above.
- There is **no `Test.describe` / `Test.it` / `Test.expect(...).toBe(...)`**.
  Those calls do not exist; the fluent request/assertion API above is the whole
  surface. Write flat assertions, one after another.
- Assertions are lazy; a bare `Test.get("/x")` with no assertion does nothing.
- Use `.equals` for whole-body matches and `.contains`/`.notContains` for
  substrings; prefer the specific one (`contains("OK")` also matches "NOT OK").
- Failing assertions report the file and the underlying `Fiber.abort` message.
