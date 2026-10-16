# HTML reference

Bialet renders HTML on the server. This page is a compact reference for the
elements, attributes and forms you reach for most; for Bialet's inline HTML
string syntax and `{{ }}` interpolation see `template.md`, and for doing more
with native HTML before reaching for JavaScript see `html-first.md`.

## Document skeleton

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Page title</title>
    <link rel="stylesheet" href="/style.css" />
  </head>
  <body>
    <main>...</main>
  </body>
</html>
```

One `<h1>` per page; keep heading levels in order (`h1` → `h2` → `h3`) without
skipping.

## Semantic sections

- `<header>`, `<nav>`, `<main>`, `<section>`, `<article>`, `<aside>`, `<footer>`.
- `<main>` wraps the unique page content; `<section>` groups a themed block and
  usually has a heading; `<article>` is self-contained (a post, a card).

## Text

`<p>`, `<strong>` (importance), `<em>` (stress), `<small>`, `<mark>`,
`<abbr title="...">`, `<code>`, `<pre>`, `<blockquote cite="...">`, `<hr>`.
`<br>` only for line breaks inside a block — never to fake spacing.

## Links

```html
<a href="/about">About</a>
<a href="/post/1" title="Read more">...</a>
<a href="#section-2">Jump</a>
<a href="/f" download>Download</a>
```

Use absolute paths (`/about`) so links work from any page depth. External links
get `rel="noopener"` when `target="_blank"`.

## Lists

```html
<ul><li>unordered</li></ul>
<ol><li>ordered</li></ol>
<dl><dt>term</dt><dd>definition</dd></dl>
```

## Images and media

```html
<img src="/pic.png" alt="description" width="640" height="480" />
<figure>
  <img src="/pic.png" alt="..." />
  <figcaption>Caption</figcaption>
</figure>
<video src="/clip.mp4" controls></video>
<audio src="/sound.mp3" controls></audio>
```

Always set `alt` (empty `alt=""` for decorative images). Setting `width` and
`height` avoids layout shift.

## Tables

```html
<table>
  <caption>Monthly totals</caption>
  <thead>
    <tr><th scope="col">Month</th><th scope="col">Total</th></tr>
  </thead>
  <tbody>
    <tr><th scope="row">Jan</th><td>120</td></tr>
  </tbody>
</table>
```

Tables are for tabular data, not layout. Use `scope` for accessibility.

## Forms

```html
<form action="/subscribe" method="post">
  <label for="email">Email</label>
  <input type="email" id="email" name="email" required autocomplete="email" />

  <label for="qty">Quantity</label>
  <input type="number" id="qty" name="qty" min="1" max="10" value="1" />

  <label for="when">Date</label>
  <input type="date" id="when" name="when" />

  <label for="note">Note</label>
  <textarea id="note" name="note" rows="4"></textarea>

  <label for="opt">Option</label>
  <select id="opt" name="opt">
    <option value="a">A</option>
    <option value="b" selected>B</option>
  </select>

  <label><input type="checkbox" name="agree" value="1" /> I agree</label>
  <label><input type="radio" name="plan" value="free" checked /> Free</label>

  <button type="submit">Send</button>
</form>
```

- Every control needs a `name` to be submitted, and a matching `<label for>`.
- `method="post"` sends a body; `method="get"` puts fields in the query string.
- Use `enctype="multipart/form-data"` when the form uploads a file.
- Native validation: `required`, `minlength`, `maxlength`, `pattern`, `min`,
  `max`, `type="email|url|number|date|tel"`.
- `novalidate` on the form disables built-in validation.
- Group related controls with `<fieldset>` + `<legend>`.
- Disabled controls (`disabled`) are not submitted; `readonly` are.

`Request.post("name")` reads a POST field, `Request.get("name")` a query
parameter, and both return `null` when the key is missing — guard with `|| ""`
before using them.

## Interactive details (no JS)

```html
<details>
  <summary>More</summary>
  <p>Hidden until opened.</p>
</details>

<dialog open>...</dialog>
```

## Accessibility essentials

- `lang` on `<html>`, meaningful `<title>`, one `<h1>`.
- Labels on every form control; `alt` on every image.
- Buttons that do an action are `<button>`; links that navigate are `<a>`.
- Don't encode meaning in color alone; keep a visible focus outline.

## Gotchas

- `{{ value }}` inside a Bialet inline HTML string is escaped; to emit raw HTML
  built by a helper, interpolate an expression, not a string.
- Inline HTML strings cannot nest the same tag. Wrap a `<div>` inside a
  `<section>` or `<article>`, not another `<div>`.
- Void elements need a space before the slash in Bialet: `<hr />`, `<input ... />`.
