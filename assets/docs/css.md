# CSS reference

Bialet pages are styled with a classless framework (Beak CSS from the jsDelivr
CDN) plus a small `style.css` of your own. This page is a compact reference for
the CSS you write by hand; for the Bialet layout conventions see `template.md`.

## Including styles

```html
<link rel="stylesheet" href="/style.css" />
```

A `<link>` in `<head>` loads before paint; a `<style>` block inlines a few rules.

## Selectors

```css
*                 /* everything */
p                 /* element */
.card             /* class */
#main             /* id (use sparingly) */
a, button         /* group */
ul li             /* descendant */
ul > li           /* direct child */
input[type=email] /* attribute */
a:hover           /* state: hover, focus, active, visited */
li:nth-child(2n)  /* structural */
:root             /* the <html> element: where variables live */
```

Prefer classes. Keep specificity low so later rules can override.

## Box model

Every element is content + `padding` + `border` + `margin`. With the modern
default:

```css
*, *::before, *::after { box-sizing: border-box; }
```

`box-sizing: border-box` makes `width` include padding and border — what you
almost always want.

```css
.box {
  width: 20rem;              /* rem = root font size, em = local font size */
  max-width: 100%;
  padding: 1rem 1.5rem;      /* top/bottom  left/right */
  margin: 0 auto;            /* auto margins center a block */
  border: 1px solid #ddd;
  border-radius: .5rem;
}
```

`display`: `block`, `inline`, `inline-block`, `none`, `flex`, `grid`.

## Custom properties (variables)

```css
:root {
  --fg: #1a1a1a;
  --bg: #ffffff;
  --accent: #2563eb;
  --radius: .5rem;
}
body { color: var(--fg); background: var(--bg); }
button { border-radius: var(--radius); background: var(--accent); }
@media (prefers-color-scheme: dark) {
  :root { --fg: #eee; --bg: #111; }
}
```

`var(--x, fallback)` supplies a default. Variables inherit, so set them on
`:root` for global theming.

## Flexbox (one dimension)

```css
.row {
  display: flex;
  gap: 1rem;                 /* space between items */
  align-items: center;       /* cross axis */
  justify-content: space-between; /* main axis */
  flex-wrap: wrap;
}
.row > .grow { flex: 1 1 auto; }   /* grow shrink basis */
```

`flex-direction: row | column` sets the main axis.

## Grid (two dimensions)

```css
.cards {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr));
  gap: 1.5rem;
}
```

`repeat(auto-fit, minmax(...))` is a responsive card grid with no media query.
Explicit tracks: `grid-template-columns: 200px 1fr;` (sidebar + content).

## Typography

```css
body {
  font: 1rem/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
  text-wrap: pretty;         /* avoid orphans/widows (modern browsers) */
}
h1 { font-size: clamp(1.75rem, 4vw, 3rem); line-height: 1.1; }
code { font-family: ui-monospace, "SF Mono", Menlo, monospace; }
```

`font: size/line-height family` is the shorthand. `clamp(min, ideal, max)` makes
fluid type.

## Color

`#rrggbb`, `#rgb`, `rgb(0 0 0 / .5)`, `hsl(220 90% 55%)`, `var(--x)`,
`transparent`, `currentColor` (inherits `color`).

## Backgrounds and borders

```css
.hero {
  background: linear-gradient(135deg, #2563eb, #7c3aed);
  background-image: url("/bg.png");
  background-size: cover;
  background-position: center;
  border: 2px dashed #999;
}
```

## Position and stacking

```css
.sticky { position: sticky; top: 0; z-index: 10; }
.overlay { position: absolute; inset: 0; }   /* inset = top/right/bottom/left */
.fixed { position: fixed; bottom: 1rem; right: 1rem; }
```

## Overflow and sizing

```css
.scroll { overflow: auto; max-height: 60vh; }
.full { width: 100%; }
.square { aspect-ratio: 1 / 1; }
```

`vh`/`vw` are viewport-relative; `rem` is relative to the root font size.

## Responsive (media queries)

```css
/* mobile-first: base styles are the small screen */
@media (min-width: 48rem) {
  .cards { grid-template-columns: repeat(3, 1fr); }
}
```

Also `@media (prefers-reduced-motion: reduce)` to disable animation.

## Transitions and animation

```css
button { transition: background-color .2s ease, transform .1s ease; }
button:hover { transform: translateY(-1px); }
@media (prefers-reduced-motion: reduce) {
  * { animation: none !important; transition: none !important; }
}
```

## States and pseudo-elements

```css
a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
input:invalid { border-color: red; }
li::before { content: "→ "; }
.clearfix::after { content: ""; display: table; clear: both; }
```

Never remove the focus outline without providing a visible alternative.

## Gotchas

- Classless frameworks style bare elements; a custom class may need
  `!important` or higher specificity to win — prefer editing your own CSS.
- `100vw` can include the scrollbar and cause horizontal scroll; prefer `100%`.
- Hard-coded pixel widths break small screens: prefer `%`, `fr`, `rem`, `clamp`.
