# JavaScript reference

Bialet is HTML-first: prefer native HTML/CSS (see `html-first.md`) and reach for
JavaScript only for what the browser cannot do alone. There is no bundler and no
package manager, so every `<script>` is a file you write and maintain by hand —
consider loading libraries from a CDN instead.

## Including a script

```html
<script src="/app.js" defer></script>
```

- `defer` runs after the HTML is parsed, in order — the right default.
- `type="module"` enables `import`/`export` and always defers.
- Put scripts in `<head>` with `defer`, or just before `</body>`.

## Variables and types

```js
const name = "Ana";        // cannot be reassigned; prefer this
let count = 0;             // reassignable
count += 1;
// var is legacy — avoid it.

typeof 1;            // "number"
typeof "x";          // "string"
typeof true;         // "boolean"
typeof undefined;    // "undefined"
typeof null;         // "object"  (historic quirk: null means "no value")
```

## Functions and scope

```js
function add(a, b) { return a + b; }
const sub = (a, b) => a - b;          // arrow function
const double = (n) => n * 2;          // single param, single expression

add(1, 2); // 3
```

Arrow functions do not bind their own `this`, which is usually what you want in
callbacks.

## Strings and template literals

```js
const greeting = `Hello, ${name}!`;
"abc".length;        // 3
"a,b,c".split(",");  // ["a", "b", "c"]
"  x  ".trim();      // "x"
"abc".includes("b"); // true
```

## Arrays

```js
const xs = [1, 2, 3];
xs.map((x) => x * 2);            // [2, 4, 6]
xs.filter((x) => x > 1);         // [2, 3]
xs.reduce((sum, x) => sum + x, 0); // 6
xs.find((x) => x === 2);         // 2
xs.includes(2);                  // true
xs.push(4);                      // mutate
const ys = [...xs, 5];           // copy + append (spread)
```

## Objects

```js
const user = { id: 1, name: "Ana" };
user.name;                       // "Ana"
const { name, id } = user;       // destructuring
const copy = { ...user, admin: false };
Object.keys(user);               // ["id", "name"]
```

## Conditionals and loops

```js
if (count > 0) { ... } else if (count === 0) { ... } else { ... }
const label = count > 0 ? "yes" : "no";
count ?? 0;                       // fallback when null/undefined
value || "default";               // fallback when falsy

for (const x of xs) { ... }       // iterate values
xs.forEach((x) => { ... });
for (let i = 0; i < xs.length; i++) { ... }
```

Use `===` / `!==` (strict equality), not `==` / `!=`.

## The DOM

```js
const el = document.querySelector(".card");        // first match
const all = document.querySelectorAll(".card");    // list

el.textContent = "Safe text";                      // escapes for you
el.innerHTML = "<b>raw</b>";                       // parses HTML — never with user input
el.classList.add("active");                        // toggle/remove too
el.setAttribute("data-x", "1");
el.value;                                          // form control value
el.remove();
```

Creating and inserting:

```js
const li = document.createElement("li");
li.textContent = "New item";
document.querySelector("ul").append(li);
```

`textContent` is safe. `innerHTML` with anything user-supplied is an XSS hole —
build nodes instead.

## Events

```js
button.addEventListener("click", (event) => {
  event.preventDefault();     // stop the default action (e.g. form submit)
  console.log("clicked");
});
```

Common events: `click`, `submit`, `input`, `change`, `keydown`, `DOMContentLoaded`.
Attach to a container and use `event.target` (delegation) instead of binding many
handlers.

## Fetch (talk to the server)

```js
const res = await fetch("/api/users");
if (!res.ok) throw new Error(`HTTP ${res.status}`);
const data = await res.json();

await fetch("/api/users", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ name: "Ana" }),
});
```

`await` needs an `async` function:

```js
async function load() {
  const res = await fetch("/api/users");
  const data = await res.json();
  // render data
}
```

## Modules

```js
import { greet } from "./utils.js";
export function greet(name) { return `Hi ${name}`; }
```

Use `<script type="module" src="/app.js"></script>`; module paths are relative
to the file and must include the `.js` extension.

## Storage

```js
localStorage.setItem("theme", "dark");   // persists
sessionStorage.getItem("theme");         // per tab
```

## Gotchas

- Keep JS out of Bialet's server-side flow: the server already owns routing and
  rendering. JS is for progressive enhancement, not the app skeleton.
- Guard DOM queries: `document.querySelector` returns `null` when nothing
  matches, and touching `.textContent` on `null` throws.
- Never build SQL or trust `innerHTML` with user input; the server is the
  boundary and must validate and escape.
