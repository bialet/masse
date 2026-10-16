# Template reference: inline HTML strings

Bialet has no template engine and no build step. HTML lives directly inside your
`.wren` files as **inline HTML strings** — a string delimited by angle brackets.
You write a `.wren` file, `return` an HTML string, and that is the page.

```wren
// index.wren
var name = "John"
return <h1>Hello, {{ name }}!</h1>
```

## Inline HTML string rules

- The string **starts and ends with the same tag**: `<p>…</p>`.
- Tag names are lowercase, start with a letter, and contain only letters,
  digits and hyphens (`<my-element>` is valid; `<MyElement>` / `<my_el>` are not).
- A string can span as many lines as you like; the newlines are part of the output.
- Void elements (`<br>`, `<img>`, `<input>`, `<meta>`, …) are written as-is. As
  the **outermost** tag, a void element must end with a space and slash
  (`<input value="{{ v }}" />`) so the parser sees the end; `<input/>` without
  the space reports "Unterminated HTML string."

## Interpolation: `{{ }}`

`{{ expr }}` evaluates any Wren expression and inserts its result:

```wren
var sum = <p>{{ 5 + 3 }}</p>            // <p>8</p>
var link = <a href="{{ url }}">{{ label }}</a>
```

**Plain values are HTML-escaped** (`&`, `<`, `>`, `"`, `'` become entities), so
you can safely interpolate user data. HTML string literals, nested `{{ }}`
results and `.raw` values are inserted verbatim. `{{ }}` must appear **inside**
an HTML tag; a `{{ }}` outside a tag is parsed as Wren map syntax.

## `return` sends the response

Inside a `.wren` file, `return` sends the value as the body and **stops
execution immediately** — nothing after it runs. `return` and the opening
`<tag>` must **not share a physical line**:

```wren
// Wrong — compilation error ("Expected expression" at return)
list.map { |x| return <div>{{ x }}</div> }

// Correct — put the tag on the line after `return`
list.map { |x|
  return <div>{{ x }}</div>
}
```

## Conditionals with `&&`

`&&` returns its right side when the left is truthy, and nothing when false:

```wren
// Toggle a class / attribute
<a class="tab {{ filter == "all" && "active" }}" href="/">All</a>

// Render a whole block conditionally
return <main>
  {{ tasks.count == 0 && <section class="empty">
    <p>Nothing here yet.</p>
  </section> }}
</main>

// Conditional text
task{{ count != 1 && "s" }} remaining
```

Inside `{{ }}` an expression may span lines, but an operator must **end** its
line — `&&`, `?`, `:` at the start of a line is a parse error:

```wren
// Valid
return <div>{{
  showClear &&
  <form method="post" action="/clear"><button>Clear</button></form>
}}</div>
```

A block callback (`map { |v| … }`) is a single expression — the body must fit on
the line after `{`, or it returns `null` and renders nothing.

## Ternaries

```wren
<button class="{{ done ? "checked" : "" }}"
        title="{{ done ? "Mark incomplete" : "Mark complete" }}"></button>

return <main>{{ done ? <span class="done">{{ text }}</span>
                  : <span class="pending">{{ text }}</span> }}</main>
```

Each branch may be a multi-line HTML string. For anything more than one level
deep, compute the value above the template and interpolate a plain variable.

## Iteration with `map`

```wren
var items = ["Apple", "Banana"]
var html = <ul>{{ items.map { |item| <li>{{ item }}</li> } }}</ul>
```

## Layout: a template class

Put shared chrome in `_app/template.wren` (a leading `_` keeps the file private)
and reuse it from every page. Prefer `static` methods so pages call them
directly:

```wren
// _app/template.wren
class Layout {
  static render(content) { Layout.render(content, "My App") }

  static render(content, title) { <!doctype html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{{ title }}</title>
        <link rel="stylesheet" href="/style.css" />
      </head>
      <body>
        <main>{{ content }}</main>
      </body>
    </html> }
}
```

```wren
// index.wren
import "/_app/template" for Layout
return Layout.render(<main><h1>Welcome</h1></main>)
```

A method body that is a single expression returns it implicitly — no `return`
needed. If you use instance methods instead of `static`, instantiate first:
`Template.new().layout(...)`, not `Template.layout(...)`.

## Pitfalls

- **The outermost tag cannot nest itself.** `<div><div>…</div></div>` fails with
  `Cannot nest <div> inside <div>`. Wrap with a different element (`<section>`,
  `<article>`, `<fieldset>`) instead of repeating the parent tag. Once the tree
  starts with a different tag, that inner tag may repeat freely.
- **Mismatched inner tags are not validated.** The parser only matches the
  outermost tag; `<div><span>Hello</div>` compiles and is served as-is (the
  browser auto-closes it). Close every tag yourself.
- **Forgetting `return`** gives an empty body — the page renders blank.
- **`_`/`.` files are private** and return 403 if fetched directly; use them for
  imports and shared logic only.
