# HTML-first reference

Bialet renders on every request — no client-side router, no hydration, no state
to sync. That model works best when the browser does what it already knows how
to do, and JavaScript is reserved for the few things it genuinely can't.
Bialet has no bundler and no package manager either: every `<script>` you add is
a file you keep by hand, so check whether HTML already does the job first.

## Disclosure: `<details>` / `<summary>`

FAQ blocks, "read more", and simple menus are a disclosure widget the browser
ships. Style the marker with `::marker` and the open state with `[open]` — no
click handler:

```wren
return <main>
  <details>
    <summary>Do I need a database?</summary>
    <p>No — Bialet works without one, but SQLite is built in if you do.</p>
  </details>
</main>
```

## Popovers and menus: the `popover` attribute

```wren
return <nav>
  <button popovertarget="user-menu">Account</button>
  <div popover id="user-menu">
    <a href="/profile">Profile</a>
    <a href="/logout">Log out</a>
  </div>
</nav>
```

Clicking outside, pressing `Esc`, or clicking another `popovertarget` closes it
automatically. Check browser support before relying on it for something critical.

## Tabs and toggles: the checkbox/radio hack

Radio buttons already track "which one is selected" — that is a tab strip. Pair
hidden inputs with CSS `:checked` and a sibling selector instead of a JS
controller:

```wren
return <div class="tabs">
  <input type="radio" name="tab" id="tab-1" checked>
  <input type="radio" name="tab" id="tab-2">
  <div class="tab-labels">
    <label for="tab-1">Overview</label>
    <label for="tab-2">Settings</label>
  </div>
  <section class="panel-1">Overview content</section>
  <section class="panel-2">Settings content</section>
</div>
```

```css
.panel-1, .panel-2 { display: none; }
#tab-1:checked ~ .panel-1, #tab-2:checked ~ .panel-2 { display: block; }
```

## Closing dialogs without JS

Opening a `<dialog>` as a true modal still needs one line of JS
(`dialog.showModal()`). Closing it doesn't — `<form method="dialog">` closes the
nearest dialog and sets `returnValue` to the button that submitted:

```wren
return <dialog id="confirm">
  <form method="dialog">
    <p>Delete this post?</p>
    <button value="cancel">Cancel</button>
    <button value="confirm">Delete</button>
  </form>
</dialog>
```

## Native form validation

`required`, `pattern`, `minlength`, `maxlength`, `min`, `max`, and typed inputs
(`type="email"`, `type="url"`, `type="number"`) validate before the request is
sent; style failures with `:invalid` / `:user-invalid`. This is convenience
only — always re-validate on the server.

```wren
return <form method="post">
  <label>Email
    <input name="email" type="email" required>
  </label>
  <button>Sign up</button>
</form>
```

## Autocomplete: `<datalist>`

Render options server-side instead of fetching on each keystroke:

```wren
var tags = `SELECT DISTINCT tag FROM posts`.fetch

return <form method="get">
  <input name="tag" list="tag-options">
  <datalist id="tag-options">
    {{ tags.map { |t| <option value="{{ t["tag"] }}"></option> } }}
  </datalist>
  <button>Filter</button>
</form>
```

## Sorting, filtering, pagination: let the server do it

Rather than ship a sort function to the client, make sorting a link that
re-requests the page with a different query param — the server re-renders anyway.
Never interpolate the sort column into the SQL string (that is both forbidden by
the Query object and an injection hole); use `.order()`, which checks the column
against an allow-list:

```wren
var sort = Request.get("sort") || "createdAt"
var dir = Request.get("order") || "asc"
var allowed = ["createdAt", "title", "views"]

var posts = `SELECT * FROM posts`.order(sort, dir, allowed).fetch

return <main>
  <nav>
    <a href="?sort=createdAt">Newest</a>
    <a href="?sort=title">Title</a>
  </nav>
  <ul>{{ posts.map { |p| <li>{{ p["title"] }}</li> } }}</ul>
</main>
```

Pagination and filtering follow the same shape: read query params, adjust the
SQL, render links to the next state. See `database.md` for `.order()` and the
parameter rules.

## When to actually reach for JS

HTML and CSS run out for anything that must react to *time* or the *server*
without a page load: search-as-you-type, drag-and-drop reordering, optimistic UI,
WebSocket updates. Write the JS for those — one small `<script>`, not a
framework. The goal is not zero JavaScript; it is not paying for it where the
browser already gives it to you.
