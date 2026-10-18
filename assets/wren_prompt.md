# Wren language guide (bundled copy)

This is the bundled copy of the Wren language guide from
[wren.io](https://wren.io), the language Bialet embeds. It is the authoritative
reference for Wren's syntax and semantics: follow it instead of guessing, and
do not invent syntax or methods it does not describe.

Bialet ships a modified dialect. Where the Bialet framework guide states a
difference (query literals, inline HTML strings, `Session`, `Test`, and so on),
the Bialet guide wins. Otherwise, the rules here apply as written.

Wren's standard library (the core classes: `Bool`, `Class`, `Fiber`, `Fn`,
`List`, `Map`, `Null`, `Num`, `Object`, `Range`, `Sequence`, `String`, `System`)
is bundled separately — read it with `bialet_docs file=wren-<class>.md`.

Contents:

- [Syntax](#syntax)
- [Values](#values)
- [Lists](#lists)
- [Maps](#maps)
- [Method Calls](#method-calls)
- [Control Flow](#control-flow)
- [Variables](#variables)
- [Classes](#classes)
- [Functions](#functions)
- [Concurrency](#concurrency)
- [Error Handling](#error-handling)
- [Modularity](#modularity)

## Syntax

Wren's syntax is designed to be familiar to people coming from C-like languages
while being a bit simpler and more streamlined.

Scripts are stored in plain text files with a `.wren` file extension. Wren does
not compile ahead of time: programs are run directly from source, from top to
bottom like a typical scripting language. (Internally, programs are compiled to
bytecode for efficiency, but that's an implementation detail.)

### Comments

Line comments start with `//` and end at the end of the line:

```wren
// This is a comment.
```

Block comments start with `/*` and end with `*/`. They can span multiple lines:

```wren
/* This
   is
   a
   multi-line
   comment. */
```

Unlike C, block comments can nest in Wren:

```wren
/* This is /* a nested */ comment. */
```

This is handy because it lets you easily comment out an entire block of code,
even if the code already contains block comments.

### Reserved words

One way to get a quick feel for a language's style is to see what words it
reserves. Here's what Wren has:

```wren
as break class construct continue else false for foreign if import
in is null return static super this true var while
```

### Identifiers

Naming rules are similar to other programming languages. Identifiers start with
a letter or underscore and may contain letters, digits, and underscores. Case
is sensitive.

```wren
hi
camelCase
PascalCase
_under_score
abc123
ALL_CAPS
```

Identifiers that start with underscore (`_`) are special in Wren. They are used
to indicate fields in classes.

### Newlines

Newlines (`\n`) are meaningful in Wren. They are used to separate statements:

```wren
// Two statements:
System.print("hi") // Newline.
System.print("bye")
```

Sometimes, though, a statement doesn't fit on a single line and jamming a
newline in the middle would trip it up. To handle that, Wren has a very simple
rule: It ignores a newline following any token that can't end a statement.

```wren
System.print( // Newline here is ignored.
    "hi")
```

In practice, this means you can put each statement on its own line and wrap
them across lines as needed without too much trouble.

### Blocks

Wren uses curly braces to define *blocks*. You can use a block anywhere a
statement is allowed, like in control flow statements.
Method and function bodies are also
blocks. For example, here we have a block for the then case, and a single
statement for the else:

```wren
if (happy && knowIt) {
  hands.clap()
} else System.print("sad")
```

Blocks have two similar but not identical forms. Typically, blocks contain a
series of statements like:

```wren
{
  System.print("one")
  System.print("two")
  System.print("three")
}
```

Blocks of this form when used for method and function bodies automatically
return `null` after the block has completed. If you want to return a different
value, you need an explicit `return` statement.

However, it's pretty common to have a method or function that just evaluates and
returns the result of a single expression. Some other languages use `=>` to
define these. Wren uses:

```wren
{ "single expression" }
```

If there is no newline after the `{` (or after the parameter list in a
function), then the block may only contain a single
expression, and it automatically returns the result of it. It's exactly the same
as doing:

```wren
{
  return "single expression"
}
```

Statements are not allowed in this form (since they don't produce values), which
means nothing starting with `class`, `for`, `if`, `import`,  `return`,
`var`, or `while`. If you want a block that contains a single statement,
put a newline in there:

```wren
{
  if (happy) {
    System.print("I'm feelin' it!")
  }
}
```

Using an initial newline after the `{` does feel a little weird or magical, but
newlines are already significant in Wren, so it's not totally unreasonable. The nice
thing about this syntax as opposed to something like `=>` is that the *end* of
the block has an explicit delimiter. That helps when chaining:

```wren
numbers.map {|n| n * 2 }.where {|n| n < 100 }
```

### Precedence and Associativity

We'll talk about Wren's different expression forms and what they mean in the
next few pages. But if you want to see how they interact with each other
grammatically, here's the whole table.

It shows which expressions have higher *precedence*—which ones bind more
tightly than others—and their *associativity*—how a series of the
same kind of expression is ordered. Wren mostly follows C, except that it fixes
the bitwise operator mistake. The full precedence table, from
tightest to loosest, is:

| Prec | Operator | Description | Associates |
|---|---|---|---|
| 1 | `()` `[]` `.` | Grouping, Subscript, Method call | Left |
| 2 | `-` `!` `~` | Negate, Not, Complement | Right |
| 3 | `*` `/` `%` | Multiply, Divide, Modulo | Left |
| 4 | `+` `-` | Add, Subtract | Left |
| 5 | `..` `...` | Inclusive range, Exclusive range | Left |
| 6 | `<<` `>>` | Left shift, Right shift | Left |
| 7 | `&` | Bitwise and | Left |
| 8 | `^` | Bitwise xor | Left |
| 9 | `\|` | Bitwise or | Left |
| 10 | `<` `<=` `>` `>=` | Comparison | Left |
| 11 | `is` | Type test | Left |
| 12 | `==` `!=` | Equals, Not equal | Left |
| 13 | `&&` | Logical and | Left |
| 14 | `\|\|` | Logical or | Left |
| 15 | `?:` | Conditional | Right |
| 16 | `=` | Assignment, Setter | Right |

## Values

Values are the built-in atomic object types that all other objects are composed
of. They can be created through *literals*, expressions that evaluate to a
value. All values are *immutable*—once created, they do not change. The
number `3` is always the number `3`. The string `"frozen"` can never have its
character array modified in place.

### Booleans

A boolean value represents truth or falsehood. There are two boolean literals,
`true` and `false`. Their class is Bool.

### Numbers

Like other scripting languages, Wren has a single numeric type:
double-precision floating point. Number literals look like you expect coming
from other languages:

```wren
0
1234
-5678
3.14159
1.0
-12.34
0.0314159e02
0.0314159e+02
314.159e-02
0xcaffe2
```

Numbers are instances of the Num class.

### Strings

A string is an array of bytes. Typically, they store characters encoded in
UTF-8, but you can put any byte values in there, even zero or invalid UTF-8
sequences. (You might have some trouble *printing* the latter to your terminal,
though.)

String literals are surrounded in double quotes:

```wren
"hi there"
```

They can also span multiple lines. When they do, the newline character within
the string will always be `\n` (`\r\n` is normalized to `\n`). 

```wren
"hi
there,
again"
```

#### Escaping

A handful of escape characters are supported:

```wren
"\0" // The NUL byte: 0.
"\"" // A double quote character.
"\\" // A backslash.
"\%" // A percent sign.
"\a" // Alarm beep. (Who uses this?)
"\b" // Backspace.
"\e" // ESC character.
"\f" // Formfeed.
"\n" // Newline.
"\r" // Carriage return.
"\t" // Tab.
"\v" // Vertical tab.

"\x48"        // Unencoded byte     (2 hex digits)
"\u0041"      // Unicode code point (4 hex digits)
"\U0001F64A"  // Unicode code point (8 hex digits)
```

A `\x` followed by two hex digits specifies a single unencoded byte:

```wren
System.print("\x48\x69\x2e") //> Hi.
```

A `\u` followed by four hex digits can be used to specify a Unicode code point:

```wren
System.print("\u0041\u0b83\u00DE") //> AஃÞ
```

A capital `\U` followed by *eight* hex digits allows Unicode code points outside
of the basic multilingual plane, like all-important emoji:

```wren
System.print("\U0001F64A\U0001F680") //> 🙊🚀
```

Strings are instances of class String.

#### Interpolation

String literals also allow *interpolation*. If you have a percent sign (`%`)
followed by a parenthesized expression, the expression is evaluated. The
resulting object's `toString` method is called and the result is inserted in the
string:

```wren
System.print("Math %(3 + 4 * 5) is fun!") //> Math 23 is fun!
```

Arbitrarily complex expressions are allowed inside the parentheses:

```wren
System.print("wow %((1..3).map {|n| n * n}.join())") //> wow 149
```

An interpolated expression can even contain a string literal which in turn has
its own nested interpolations, but doing that gets unreadable pretty quickly.

#### Raw strings

A string literal can also be created using triple quotes `"""` which is
parsed as a raw string. A raw string is no different
from any other string, it's just parsed in a different way.

**Raw strings do not process escapes and do not apply any interpolation**.

```wren
"""hi there"""
```

When a raw string spans multiple lines and a triple quote is on it's own line,
any whitespace on that line will be ignored. This means the opening and closing
lines are not counted as part of the string when the triple quotes are separate lines,
as long as they only contain whitespace (spaces + tabs).

```wren
  """
    Hello world
  """
```

The resulting value in the string above has no newlines or trailing whitespace. 
Note the spaces in front of the Hello are preserved. 

```wren
    Hello world
```

A raw string will be parsed exactly as is in the file, unmodified.
This means it can contain quotes, invalid syntax, other data formats 
and so on without being modified by Wren.

```wren
"""
  {
    "hello": "wren",
    "from" : "json"
  }
"""
```

One more example, embedding wren code inside a string safely.

```wren
"""
A markdown string with embedded wren code example.

    class Example {
      construct code() {
        //
      }
    }
"""
```

### Ranges

A range is a little object that represents a consecutive range of numbers. They
don't have their own dedicated literal syntax. Instead, the number class
implements the `..` and `...` operators to create them:

```wren
3..8
```

This creates a range from three to eight, including eight itself. If you want a
half-inclusive range, use `...`:

```wren
4...6
```

This creates a range from four to six *not* including six itself. Ranges are
commonly used for iterating over a
sequences of numbers, but are useful in other places too. You can pass them to
a list's subscript operator to return a subset of the list, for
example, or on a String, the substring in that range:

```wren
var list = ["a", "b", "c", "d", "e"]
var slice = list[1..3]
System.print(slice) //> [b, c, d]

var string = "hello wren"
var wren = string[-4..-1]
System.print(wren) //> wren
```

Their class is Range.

### Null

Wren has a special value `null`, which is the only instance of the class
Null. (Note the difference in case.) It functions a bit like `void` in some
languages: it indicates the absence of a value. If you call a method that
doesn't return anything and get its returned value, you get `null` back.

## Lists

A list is a compound object that holds a collection of elements identified by
integer index. You can create a list by placing a sequence of comma-separated
expressions inside square brackets:

```wren
[1, "banana", true]
```

Here, we've created a list of three elements. Notice that the elements don't
have to be the same type.

### Accessing elements

You can access an element from a list by calling the subscript
operator on it with the index of the
element you want. Like most languages, indexes start at zero:

```wren
var trees = ["cedar", "birch", "oak", "willow"]
System.print(trees[0]) //> cedar
System.print(trees[1]) //> birch
```

Negative indices counts backwards from the end:

```wren
System.print(trees[-1]) //> willow
System.print(trees[-2]) //> oak
```

It's a runtime error to pass an index outside of the bounds of the list. If you
don't know what those bounds are, you can find out using count:

```wren
System.print(trees.count) //> 4
```

### Slices and ranges

Sometimes you want to copy a chunk of elements from a list. You can do that by
passing a range to the subscript operator, like so:

```wren
System.print(trees[1..2]) //> [birch, oak]
```

This returns a new list containing the elements of the original list whose
indices are within the given range. Both inclusive and exclusive ranges work
and do what you expect.

Negative bounds also work like they do when passing a single number, so to copy
a list, you can just do:

```wren
trees[0..-1]
```

### Adding elements

Lists are *mutable*, meaning their contents can be changed. You can swap out an
existing element in the list using the subscript setter:

```wren
trees[1] = "spruce"
System.print(trees[1]) //> spruce
```

It's an error to set an element that's out of bounds. To grow a list, you can
use `add` to append a single item to the end:

```wren
trees.add("maple")
System.print(trees.count) //> 5
```

You can insert a new element at a specific position using `insert`:

```wren
trees.insert(2, "hickory")
```

The first argument is the index to insert at, and the second is the value to
insert. All elements following the inserted one will be pushed down to
make room for it.

It's valid to "insert" after the last element in the list, but only *right*
after it. Like other methods, you can use a negative index to count from the
back. Doing so counts back from the size of the list *after* it's grown by one:

```wren
var letters = ["a", "b", "c"]
letters.insert(3, "d")   // OK: inserts at end.
System.print(letters)    //> [a, b, c, d]
letters.insert(-2, "e")  // Counts back from size after insert.
System.print(letters)    //> [a, b, c, e, d]
```

### Adding lists together

Lists have the ability to be added together via the `+` operator. This is often known as concatenation.

```wren
var letters = ["a", "b", "c"]
var other = ["d", "e", "f"]
var combined = letters + other
System.print(combined)  //> [a, b, c, d, e, f]
```

### Removing elements

The opposite of `insert` is `removeAt`. It removes a single element from a
given position in the list. 

To remove a specific _value_ instead, use `remove`. The first value that 
matches using regular equality will be removed.

In both cases, all following items are shifted up to fill in the gap.

```wren
var letters = ["a", "b", "c", "d"]
letters.removeAt(1)
System.print(letters) //> [a, c, d]
letters.remove("a")
System.print(letters) //> [c, d]
```

Both the `remove` and `removeAt` method return the removed item:

```wren
System.print(letters.removeAt(1)) //> c
```

If `remove` couldn't find the value in the list, it returns null:

```wren
System.print(letters.remove("not found")) //> null
```

If you want to remove everything from the list, you can clear it:

```wren
trees.clear()
System.print(trees) //> []
```

## Maps

A map is an *associative* collection. It holds a set of entries, each of which
maps a *key* to a *value*. The same data structure has a variety of names in
other languages: hash table, dictionary, association, table, etc.

You can create a map by placing a series of comma-separated entries inside
curly braces. Each entry is a key and a value separated by a colon:

```wren
{
  "maple":  "Sugar Maple (Acer Saccharum)",
  "larch":  "Alpine Larch (Larix Lyallii)",
  "oak":    "Red Oak (Quercus Rubra)",
  "fir":    "Fraser Fir (Abies Fraseri)"
}
```

This creates a map that associates a type of tree (key) to a specific 
tree within that family (value). Syntactically, in a map literal, keys 
can be any literal, a variable name, or a parenthesized expression. 
Values can be any expression. Here, we're using string literals for both keys 
and values.

*Semantically*, values can be any object, and multiple keys may map to the same
value. 

Keys have a few limitations. They must be one of the immutable built-in
value types in Wren. That means a number, string, range, bool, or `null`.
You can also use a class object as a key (not an instance of that class, 
the actual class itself).

The reason for this limitation—and the reason maps are called "*hash*
tables" in other languages—is that each key is used to generate a numeric
*hash code*. This lets a map locate the value associated with a key in constant
time, even in very large maps. Since Wren only knows how to hash certain
built-in types, only those can be used as keys.

### Adding entries

You add new key-value pairs to the map using the subscript operator:

```wren
var capitals = {}
    capitals["Georgia"] = "Atlanta"
    capitals["Idaho"] = "Boise"
    capitals["Maine"] = "Augusta"
```

If the key isn't already present, this adds it and associates it with the given
value. If the key is already there, this just replaces its value.

### Looking up values

To find the value associated with some key, again you use your friend the
subscript operator:

```wren
System.print(capitals["Idaho"]) //> Boise
```

If the key is present, this returns its value. Otherwise, it returns `null`. Of
course, `null` itself can also be used as a value, so seeing `null` here
doesn't necessarily mean the key wasn't found.

To tell definitively if a key exists, you can call `containsKey()`:

```wren
var capitals = {"Georgia": null}

System.print(capitals["Georgia"]) //> null (though key exists)
System.print(capitals["Idaho"])   //> null 
System.print(capitals.containsKey("Georgia")) //> true
System.print(capitals.containsKey("Idaho"))   //> false
```

You can see how many entries a map contains using `count`:

```wren
System.print(capitals.count) //> 3
```

### Removing entries

To remove an entry from a map, call `remove()` and pass in the key for the
entry you want to delete:

```wren
capitals.remove("Maine")
System.print(capitals.containsKey("Maine")) //> false
```

If the key was found, this returns the value that was associated with it:

```wren
System.print(capitals.remove("Georgia")) //> Atlanta
```

If the key wasn't in the map to begin with, `remove()` just returns `null`.

If you want to remove *everything* from the map, like with lists, you call
`clear()`:

```wren
capitals.clear()
System.print(capitals.count) //> 0
```

### Iterating over the contents

The subscript operator works well for finding values when you know the key
you're looking for, but sometimes you want to see everything that's in the map.
You can use a regular for loop to iterate the contents, and map exposes two 
additional methods to access the contents: `keys` and `values`. 

The `keys` method on a map returns a Sequence that iterates over all of
the keys in the map, and the `values` method returns one that iterates over the values.

Regardless of how you iterate, the *order* that things are iterated in 
isn't defined. Wren makes no promises about what order keys and values are 
iterated. All it promises is that every entry will appear exactly once.

**Iterating with for(entry in map)**   
When you iterate a map with `for`, you'll be handed an _entry_, which contains
a `key` and a `value` field. That gives you the info for each element in the map.

```wren
var birds = {
  "Arizona": "Cactus wren",
  "Hawaii": "Nēnē",
  "Ohio": "Northern Cardinal"
}

for (bird in birds) {
  System.print("The state bird of %(bird.key) is %(bird.value)")
}
```

**Iterating using the keys**   

You can also iterate over the keys and use each to look up its value:

```wren
var birds = {
  "Arizona": "Cactus wren",
  "Hawaii": "Nēnē",
  "Ohio": "Northern Cardinal"
}

for (state in birds.keys) {
  System.print("The state bird of %(state) is " + birds[state])
}
```

## Method Calls

Wren is deeply object oriented, so most code consists of invoking methods on
objects, usually something like this:

```wren
System.print("Heyoo!") //> Heyoo!
```

You have a *receiver* expression (here `System`) followed by a `.`, then a name
(`print`) and an argument list in parentheses (`("Heyoo!")`). Multiple arguments
are separated by commas:

```wren
list.insert(3, "item")
```

The argument list can also be empty:

```wren
list.clear()
```

The VM executes a method call like so:

1. Evaluate the receiver and arguments from left to right.
2. Look up the method on the receiver's class.
3. Invoke it, passing in the argument values.

### Signature

Unlike most other dynamically-typed languages, in Wren a class can have multiple
methods with the same *name*, as long as they have different *signatures*. The
signature includes the method's name along with the number of arguments it
takes. In technical terms, this means you can *overload by arity*.

For example, the Random class has two methods for getting a random integer.
One takes a minimum and maximum value and returns a value in that range. The
other only takes a maximum value and uses 0 as the minimum:

```wren
var random = Random.new()
random.int(3, 10)
random.int(4)
```

In a language like Python or JavaScript, these would both call a single `int()`
method, which has some kind of "optional" parameter. The body of the method
figures out how many arguments were passed and uses control flow to handle the
two different behaviors. That means first parameter represents "max unless
another parameter was passed, in which case it's min". 

This type of 'variadic' code isn't ideal, so Wren doesn't encourage it.

In Wren, these are calls to two entirely separate methods, `int(_,_)` and
`int(_)`. This makes it easier to define "overloads" like this since you don't
need optional parameters or any kind of control flow to handle the different
cases.

It's also faster to execute. Since we know how many arguments are passed at
compile time, we can compile this to directly call the right method and avoid
any "if I got two arguments do this..." runtime work.

### Getters

Some methods exist to expose a stored or computed property of an object. These
are *getters* and have no parentheses:

```wren
"string".count    //> 6
(1..10).min       //> 1
1.23.sin          //> 0.9424888019317
[1, 2, 3].isEmpty //> false
```

A getter is *not* the same as a method with an empty argument list. The `()` is
part of the signature, so `count` and `count()` have different signatures.
Unlike Ruby's optional parentheses, Wren wants to make sure you call a getter
like a getter and a `()` method like a `()` method. These don't work:

```wren
"string".count()
[1, 2, 3].clear
```

If you're defining some member that doesn't need any parameters, you need to
decide if it should be a getter or a method with an empty `()` parameter list.
The general guidelines are:

*   If it modifies the object or has some other side effect, make it a method:

```wren
list.clear()
```

*   If the method supports multiple arities, make the zero-parameter case a `()`
    method to be consistent with the other versions:

```wren
Fiber.yield()
Fiber.yield("value")
```

*   Otherwise, it can probably be a getter.

### Setters

A getter lets an object expose a public "property" that you can *read*.
Likewise, a *setter* lets you write to a property:

```wren
person.height = 74 // Grew up!
```

Despite the `=`, this is just another syntax for a method call. From the
language's perspective, the above line is just a call to the `height=(_)`
method on `person`, passing in `74`.

Since the `=(_)` is in the setter's signature, an object can have both a getter
and setter with the same name without a collision. Defining both lets you
provide a read/write property.

### Operators

Wren has most of the same operators you know and love with the same precedence
and associativity. We have three prefix operators:

```wren
! ~ -
```

They are just method calls on their operand without any other arguments. An
expression like `!possible` means "call the `!` method on `possible`".

We also have a slew of infix operators—they have operands on both sides.
They are:

```wren
* / % + - .. ... << >> < <= > >= == != & ^ | is
```

Like prefix operators, they are all funny ways of writing method calls. The left
operand is the receiver, and the right operand gets passed to it. So `a + b` is
semantically interpreted as "invoke the `+(_)` method on `a`, passing it `b`".

Note that `-` is both a prefix and an infix operator. Since they have different
signatures (`-` and `-(_)`), there's no ambiguity between them.

Most of these are probably familiar already. The `..` and `...` operators are
"range" operators. The number type implements those to create range
objects, but they are method calls like other operators.

The `is` keyword is a "type test" operator. The base Object class implements
it to tell if an object is an instance of a given class. You'll rarely need to,
but you can override `is` in your own classes. That can be useful for things
like mocks or proxies where you want an object to masquerade as a certain class.

### Subscripts

Another familiar syntax from math is *subscripting* using square brackets
(`[]`). It's handy for working with collection-like objects. For example:

```wren
list[0]    // Get the first item in a list.
map["key"] // Get the value associated with "key".
```

You know the refrain by now. In Wren, these are method calls. In the above
examples, the signature is `[_]`. Subscript operators may also take multiple
arguments, which is useful for things like multi-dimensional arrays:

```wren
matrix[3, 5]
```

These examples are subscript "getters", and there are also
corresponding *subscript setters*:

```wren
list[0] = "item"
map["key"] = "value"
```

These are equivalent to method calls whose signature is `[_]=(_)` and whose
arguments are both the subscript (or subscripts) and the value on the right-hand
side.

## Control Flow

Control flow is used to determine which chunks of code are executed and how many
times. *Branching* statements and expressions decide whether or not to execute
some code and *looping* ones execute something more than once.

### Truth

All control flow is based on *deciding* whether or not to do something. This
decision depends on some expression's value. We take the entire universe of
possible objects and divide them into two buckets: some we consider "true" and
the rest are "false". If the expression results in a value in the true bucket,
we do one thing. Otherwise, we do something else.

Obviously, the boolean `true` is in the "true" bucket and `false` is in
"false", but what about values of other types? The choice is ultimately
arbitrary, and different languages have different rules. Wren's rules follow
Ruby:

  * The boolean value `false` is false.
  * The null value `null` is false.
  * Everything else is true.

This means `0`, empty strings, and empty collections are all considered "true"
values.

### If statements

The simplest branching statement, `if` lets you conditionally skip a chunk of
code. It looks like this:

```wren
if (ready) System.print("go!")
```

That evaluates the parenthesized expression after `if`. If it's true, then the
statement after the condition is evaluated. Otherwise it is skipped. Instead of
a statement, you can have a block:

```wren
if (ready) {
  System.print("getSet")
  System.print("go!")
}
```

You may also provide an `else` branch. It will be executed if the condition is
false:

```wren
if (ready) System.print("go!") else System.print("not ready!")
```

And, of course, it can take a block too:

```wren
if (ready) {
  System.print("go!")
} else {
  System.print("not ready!")
}
```

You can also use `else if` branches to handle multiple possibilities. For example, what if `ready` was not a boolean value?
```wren
if (ready == true) {
  System.print("go!")
} else if (ready == false) {
  System.print("not ready!")
} else { // If ready isn't a boolean
  System.print("not sure if I'm ready or not!")
}
```

### Logical operators

Unlike most other operators in Wren which are just a special syntax for
method calls, the `&&` and `||` operators are special. This is because they
only conditionally evaluate the right operand—they short-circuit.

A `&&` ("logical and") expression evaluates the left-hand argument. If it's
false, it returns that value. Otherwise it evaluates and returns the right-hand
argument.

```wren
System.print(false && 1)  //> false
System.print(1 && 2)      //> 2
```

A `||` ("logical or") expression is reversed. If the left-hand argument is
*true*, it's returned, otherwise the right-hand argument is evaluated and
returned:

```wren
System.print(false || 1)  //> 1
System.print(1 || 2)      //> 1
```

### The conditional operator `?:`

Also known as the "ternary" operator since it takes three arguments, Wren has
the little "if statement in the form of an expression" you know and love from C
and similar languages.

```wren
System.print(1 != 2 ? "math is sane" : "math is not sane!")
```

It takes a condition expression, followed by `?`, followed by a then
expression, a `:`, then an else expression. Just like `if`, it evaluates the
condition. If true, it evaluates and returns the then expression. Otherwise
it does the else expression.

### While statements

It's hard to write a useful program without executing some chunk of code
repeatedly. To do that, you use looping statements. There are two in Wren, and
they should be familiar if you've used other imperative languages.

The simplest, a `while` statement executes a chunk of code as long as a
condition continues to hold. For example:

```wren
// Hailstone sequence.
var n = 27
while (n != 1) {
  if (n % 2 == 0) {
    n = n / 2
  } else {
    n = 3 * n + 1
  }
}
```

This evaluates the expression `n != 1`. If it is true, then it executes the
following body. After that, it loops back to the top, and evaluates the
condition again. It keeps doing this as long as the condition evaluates to
something true.

The condition for a while loop can be any expression, and must be surrounded by
parentheses. The body of the loop is usually a curly block but can also be a
single statement:

```wren
var n = 27
while (n != 1) if (n % 2 == 0) n = n / 2 else n = 3 * n + 1
```

### For statements

While statements are useful when you want to loop indefinitely or according to
some complex condition. But in most cases, you're looping through
a list, a series of numbers, or some other "sequence" object.
That's what `for` is, uh, for. It looks like this:

```wren
for (beatle in ["george", "john", "paul", "ringo"]) {
  System.print(beatle)
}
```

A `for` loop has three components:

1. A *variable name* to bind. In the example, that's `beatle`. Wren will create
   a new variable with that name whose scope is the body of the loop.

2. A *sequence expression*. This determines what you're looping over. It gets
   evaluated *once* before the body of the loop. In this case, it's a list
   literal, but it can be any expression.

3. A *body*. This is a curly block or a single statement. It gets executed once
   for each iteration of the loop.

### Break statements

Sometimes, right in the middle of a loop body, you decide you want to bail out
and stop. To do that, you can use a `break` statement. It's just the `break`
keyword all by itself. That immediately exits out of the nearest enclosing
`while` or `for` loop.

```wren
for (i in [1, 2, 3, 4]) {
  System.print(i)           //> 1
  if (i == 3) break         //> 2
}                           //> 3
```

### Continue statements

During the execution of a loop body, you might decide that you want to skip the 
rest of this iteration and move on to the next one. You can use a `continue` 
statement to do that. It's just the `continue` keyword all by itself. Execution
will immediately jump to the beginning of the next loop iteration (and check the
loop conditions).

```wren
for (i in [1, 2, 3, 4]) {
  if (i == 2) continue      //> 1
  System.print(i)           //> 3
}                           //> 4
```

### Numeric ranges

Lists are one common use for `for` loops, but sometimes you want to walk over a
sequence of numbers, or loop a number of times. For that, you can create a
range, like so:

```wren
for (i in 1..100) {
  System.print(i)
}
```

This loops over the numbers from 1 to 100, including 100 itself. If you want to
leave off the last value, use three dots instead of two:

```wren
for (i in 1...100) {
  System.print(i)
}
```

This looks like some special "range" syntax in the `for` loop, but it's actually
just a pair of operators. The `..` and `...` syntax are infix "range" operators.
Like other operators, they are special syntax for a regular method
call. The number type implements them and returns a range object that knows
how to iterate over a series of numbers.

### The iterator protocol

Lists and ranges cover the two most common kinds of loops, but you should also
be able to define your own sequences. To enable that, the semantics of `for`
are defined in terms of an "iterator protocol". The loop itself doesn't know
anything about lists or ranges, it just knows how to call two particular
methods on the object that resulted from evaluating the sequence expression.

When you write a loop like this:

```wren
for (i in 1..100) {
  System.print(i)
}
```

Wren sees it something like this:

```wren
var iter_ = null
var seq_ = 1..100
while (iter_ = seq_.iterate(iter_)) {
  var i = seq_.iteratorValue(iter_)
  System.print(i)
}
```

First, Wren evaluates the sequence expression and stores it in a hidden
variable (written `seq_` in the example but in reality it doesn't have a name
you can use). It also creates a hidden "iterator" variable and initializes it
to `null`.

Each iteration, it calls `iterate()` on the sequence, passing in the current
iterator value. (In the first iteration, it passes in `null`.) The sequence's
job is to take that iterator and advance it to the next element in the
sequence. (Or, in the case where the iterator is `null`, to advance it to the
*first* element). It then returns either the new iterator, or `false` to
indicate that there are no more elements.

If `false` is returned, Wren exits out of the loop and we're done. If anything
else is returned, that means that we have advanced to a new valid element. To
get that, Wren then calls `iteratorValue()` on the sequence and passes in the
iterator value that it just got from calling `iterate()`. The sequence uses
that to look up and return the appropriate element.

The built-in List and Range types implement
`iterate()` and `iteratorValue()` to walk over their respective sequences. You
can implement the same methods in your classes to make your own types iterable.

## Variables

Variables are named slots for storing values. You define a new variable in Wren
using a `var` statement, like so:

```wren
var a = 1 + 2
```

This creates a new variable `a` in the current scope and initializes it with
the result of the expression following the `=`. Once a variable has been
defined, it can be accessed by name as you would expect.

```wren
var animal = "Slow Loris"
System.print(animal) //> Slow Loris
```

### Scope

Wren has true block scope: a variable exists from the point where it is defined
until the end of the block where that definition appears.

```wren
{
  System.print(a) //! "a" doesn't exist yet.
  var a = 123
  System.print(a) //> 123
}
System.print(a) //! "a" doesn't exist anymore.
```

Variables defined at the top level of a script are *top-level* and are visible
to the module system. All other variables are *local*.
Declaring a variable in an inner scope with the same name as an outer one is
called *shadowing* and is not an error (although it's not something you likely
intend to do much).

```wren
var a = "outer"
{
  var a = "inner"
  System.print(a) //> inner
}
System.print(a) //> outer
```

Declaring a variable with the same name in the *same* scope *is* an error.

```wren
var a = "hi"
var a = "again" //! "a" is already declared.
```

### Assignment

After a variable has been declared, you can assign to it using `=`

```wren
var a = 123
a = 234
```

An assignment walks up the scope stack to find where the named variable is
declared. It's an error to assign to a variable that isn't defined. Wren
doesn't roll with implicit variable definition.

When used in a larger expression, an assignment expression evaluates to the
assigned value.

```wren
var a = "before"
System.print(a = "after") //> after
```

If the left-hand side is some more complex expression than a bare variable name,
then it isn't an assignment. Instead, it's calling a setter method.

## Classes

Every value in Wren is an object, and every object is an instance of a class.
Even `true` and `false` are full-featured objects—instances of the
Bool class.

Classes define an objects *behavior* and *state*. Behavior is defined by
*methods* which live in the class. Every object of the same
class supports the same methods. State is defined in *fields*, whose values are
stored in each instance.

### Defining a class

Classes are created using the `class` keyword, unsurprisingly:

```wren
class Unicorn {}
```

This creates a class named `Unicorn` with no methods or fields.

### Methods

To let our unicorn do stuff, we need to give it methods.

```wren
class Unicorn {
  prance() {
    System.print("The unicorn prances in a fancy manner!")
  }
}
```

This defines a `prance()` method that takes no arguments. To add parameters, put
their names inside the parentheses:

```wren
class Unicorn {
  prance(where, when) {
    System.print("The unicorn prances in %(where) at %(when).")
  }
}
```

Since the number of parameters is part of a method's signature a class can
define multiple methods with the same name:

```wren
class Unicorn {
  prance() {
    System.print("The unicorn prances in a fancy manner!")
  }

  prance(where) {
    System.print("The unicorn prances in %(where).")
  }

  prance(where, when) {
    System.print("The unicorn prances in %(where) at %(when).")
  }
}
```

It's often natural to have the same conceptual operation work with different
sets of arguments. In other languages, you'd define a single method for the
operation and have to check for missing optional arguments. In Wren, they are
different methods that you implement separately.

In addition to named methods with parameter lists, Wren has a bunch of other
different syntaxes for methods. Your classes can define all of them.

#### Getters

A getter leaves off the parameter list and the parentheses:

```wren
class Unicorn {
  // Unicorns are always fancy.
  isFancy { true }
}
```

#### Setters

A setter has `=` after the name, followed by a single parenthesized parameter:

```wren
class Unicorn {
  rider=(value) {
    System.print("I am being ridden by %(value).")
  }
}
```

By convention, the parameter is usually named `value` but you can call it
whatever makes your heart flutter.

#### Operators

Prefix operators, like getters, have no parameter list:

```wren
class Unicorn {
  - {
    System.print("Negating a unicorn is weird.")
  }
}
```

Infix operators, like setters, have a single parenthesized parameter for the
right-hand operand:

```wren
class Unicorn {
  -(other) {
    System.print("Subtracting %(other) from a unicorn is weird.")
  }
}
```

A subscript operator puts the parameters inside square brackets and can have
more than one:

```wren
class Unicorn {
  [index] {
    System.print("Unicorns are not lists!")
  }

  [x, y] {
    System.print("Unicorns are not matrices either!")
  }
}
```

Unlike with named methods, you can't define a subscript operator with an empty
parameter list.

As the name implies, a subscript setter looks like a combination of a subscript
operator and a setter:

```wren
class Unicorn {
  [index]=(value) {
    System.print("You can't stuff %(value) into me at %(index)!")
  }
}
```

### Method Scope

Up to this point, "scope" has been used to talk exclusively about
variables. In a procedural language like C, or a functional one like Scheme,
that's the only kind of scope there is. But object-oriented languages like Wren
introduce another kind of scope: *object scope*. It contains the methods that
are available on an object. When you write:

```wren
unicorn.isFancy
```

You're saying "look up the method `isFancy` in the scope of the object
`unicorn`”. In this case, the fact that you want to look up a *method*
`isFancy` and not a *variable* `isFancy` is explicit. That's what `.` does and
the object to the left of the period is the object you want to look up the
method on.

#### `this`

Things get more interesting when you're inside the body of a method. When the
method is called on some object and the body is being executed, you often need
to access that object itself. You can do that using `this`.

```wren
class Unicorn {
  name { "Francis" }

  printName() {
    System.print(this.name) //> Francis
  }
}
```

The `this` keyword works sort of like a variable, but has special behavior. It
always refers to the instance whose method is currently being executed. This
lets you invoke methods on "yourself".

It's an error to refer to `this` outside of a method. However, it's perfectly
fine to use it inside a function declared *inside* a method. When you do,
`this` still refers to the instance whose *method* is being called:

```wren
class Unicorn {
  name { "Francis" }

  printNameThrice() {
    (1..3).each {
      // Use "this" inside the function passed to each().
      System.print(this.name) //> Francis
    } //> Francis
  } //> Francis
}
```

This is unlike Lua and JavaScript which can "forget" `this` when you create a
callback inside a method. Wren does what you want here and retains the
reference to the original object.

(In technical terms, a function's closure includes `this`. Wren can do this
because it makes a distinction between methods and functions.)

#### Implicit `this`

Using `this.` every time you want to call a method on yourself works, but it's
tedious and verbose, which is why some languages don't require it. You can do a
"self send" by calling a method (or getter or setter) without any explicit
receiver:

```wren
class Unicorn {
  name { "Francis" }

  printName() {
    System.print(name) //> Francis
  }
}
```

Code like this gets tricky when there is also a variable outside of the class
with the same name. Consider:

```wren
var name = "variable"

class Unicorn {
  name { "Francis" }

  printName() {
    System.print(name) // ???
  }
}
```

Should `printName()` print "variable" or "Francis"? A method body has a foot in
each of two worlds. It is surrounded by the lexical scope where it's defined in
the program, but it also has the object scope of the methods on `this`.

Which scope wins? Every language has to decide how to handle this and there
is a surprising plethora of approaches. Wren's approach to resolving a name
inside a method works like this:

1.  If there is a local variable inside the method with that name, that wins.
2.  Else, if the name starts with a lowercase letter, treat it like a method on
    `this`.
3.  Otherwise, look for a variable with that name in the surrounding scope.

So, in the above example, we hit case #2 and it prints "Francis". Distinguishing
self sends from outer variables based on the *case* of the first letter in the
name probably seems weird but it works surprisingly well. Method names are
lowercase in Wren. Class names are capitalized.

Most of the time, when you're in a method and want to access a name from outside
of the class, it's usually the name of some other class. This rule makes that
work.

Here's an example that shows all three cases:

```wren
var shadowed = "surrounding"
var lowercase = "surrounding"
var Capitalized = "surrounding"

class Scope {
  shadowed { "object" }
  lowercase { "object" }
  Capitalized { "object" }

  test() {
    var shadowed = "local"

    System.print(shadowed) //> local
    System.print(lowercase) //> object
    System.print(Capitalized) //> surrounding
  }
}
```

It's a bit of a strange rule, but Ruby works more or less the same way.

### Constructors

We've seen how to define kinds of objects and how to declare methods on them.
Our unicorns can prance around, but we don't actually *have* any unicorns to do
it. To create *instances* of a class, we need a *constructor*. You define one
like so:

```wren
class Unicorn {
  construct new(name, color) {
    System.print("My name is " + name + " and I am " + color + ".")
  }
}
```

The `construct` keyword says we're defining a constructor, and `new` is its
name. In Wren, all constructors have names. The word "new" isn't special to
Wren, it's just a common constructor name.

To make a unicorn now, we call the constructor method on the class itself:

```wren
var fred = Unicorn.new("Fred", "palomino")
```

Giving constructors names is handy because it means you can have more than one,
and each can clarify how it creates the instance:

```wren
class Unicorn {
  construct brown(name) {
    System.print("My name is " + name + " and I am brown.")
  }
}

var dave = Unicorn.brown("Dave")
```

Note that we have to declare a constructor because, unlike some other
languages, Wren doesn't give you a default one. This is useful because some
classes aren't designed to be constructed. If you have an abstract base class
that just contains methods to be inherited by other classes, it doesn't need
and won't have a constructor.

Like other methods, constructors can obviously have arguments, and can be
overloaded by [arity](#signature). A constructor *must* be a named method with
a (possibly empty) argument list. Operators, getters, and setters cannot be
constructors.

A constructor returns the instance of the class being created, even if you 
don't explicitly use `return`. It is valid to use `return` inside of a 
constructor, but it is an error to have an expression after the return.
That rule applies to `return this` as well, return handles that implicitly inside
a constructor, so just `return` is enough.

```wren
return          //> valid, returns 'this'

return variable //> invalid
return null     //> invalid
return this     //> also invalid
```

A constructor is actually a pair of methods. You get a method on the class:

```wren
Unicorn.brown("Dave")
```

That creates the new instance, then it invokes the *initializer* on that
instance. This is where the constructor body you defined gets run.

This distinction is important because it means inside the body of the
constructor, you can access `this`, assign [fields](#fields), call superclass
constructors, etc.

### Fields

All state stored in instances is stored in *fields*. Each field has a name
that starts with an underscore.

```wren
class Rectangle {
  area { _width * _height }

  // Other stuff...
}
```

Here, `_width` and `_height` in the `area` getter refer
to fields on the rectangle instance. You can think of them like `this.width`
and `this.height` in other languages.

When a field name appears, Wren looks for the nearest enclosing class and looks
up the field on the instance of that class. Field names cannot be used outside
of an instance method. They *can* be used inside a function
in a method. Wren will look outside any nested functions until it finds an
enclosing method.

Unlike variables, fields are implicitly declared by simply
assigning to them. If you access a field before it has been initialized, its
value is `null`.

#### Encapsulation

All fields are *private* in Wren—an object's fields can only be directly
accessed from within methods defined on the object's class. 

In short, if you want to make a property of an object visible,
**you need to define a getter to expose it**:

```wren
class Rectangle {
  width { _width }
  height { _height }

  // ...
}
```

To allow outside code to modify the field,
**you need to provide setters to provide access**:

```wren
class Rectangle {
  width=(value) { _width = value }
  height=(value) { _height = value }
}
```

This might be different from what you're used to, so here are two important facts:

- You can't access fields from a base class.
- You can't access fields on another instance of your own class.

Here is an example in code:

```wren
class Shape {
  construct new() {
    _shape = "none"
  }
}

class Rectangle is Shape {
  construct new() {
    //This will print null!
    //_shape from the parent class is private,
    //we are reading `_shape` from `this`,
    //which has not been set, so returns null.
    System.print("I am a %(_shape)")

    //a local variable, all variables are private
    _width = 10
    var other = Rectangle.new()

    //other._width is not accessible from here,
    //even though we are also a rectangle. The field
    //is private, and other._width is invalid syntax!
  }
}
...
```

One thing we've learned in the past forty years of software engineering is that
encapsulating state tends to make code easier to maintain, so Wren defaults to
keeping your object's state pretty tightly bundled up. Don't feel that you have
to or even should define getters or setters for most of your object's fields.

### Metaclasses and static members

#### Static fields

A name that starts with *two* underscores is a *static* field. They work
similar to [fields](#fields) except the data is stored on the class itself, and
not the instance. They can be used in *both* instance and static methods.

```wren
class Foo {
  construct new() {}

  static setFromStatic(a) { __a = a }
  setFromInstance(a) { __a = a }

  static printFromStatic() {
    System.print(__a)
  }

  printFromInstance() {
    System.print(__a)
  }
}
```

Just like instance fields, static fields are initially `null`:

```wren
Foo.printFromStatic() //> null
```

They can be used from static methods:

```wren
Foo.setFromStatic("first")
Foo.printFromStatic() //> first
```

And also instance methods. When you do so, there is still only one static field
shared among all instances of the class:

```wren
var foo1 = Foo.new()
var foo2 = Foo.new()

foo1.setFromInstance("second")
foo2.printFromInstance() //> second
```

### Inheritance

A class can inherit from a "parent" or *superclass*. When you invoke a method
on an object of some class, if it can't be found, it walks up the chain of
superclasses looking for it there.

By default, any new class inherits from Object, which is the superclass from
which all other classes ultimately descend. You can specify a different parent
class using `is` when you declare the class:

```wren
class Pegasus is Unicorn {}
```

This declares a new class Pegasus that inherits from Unicorn.

Note that you should not create classes that inherit from the built-in types
(Bool, Num, String, Range, List). The built-in types expect their internal bit
representation to be very specific and get horribly confused when you invoke one
of the inherited built-in methods on the derived type.

The metaclass hierarchy does *not* parallel the regular class hierarchy. So, if
Pegasus inherits from Unicorn, Pegasus's metaclass does not inherit from
Unicorn's metaclass. In more prosaic terms, this means that static methods are
not inherited.

```wren
class Unicorn {
  // Unicorns cannot fly. :(
  static canFly { false }
}

class Pegasus is Unicorn {}

Pegasus.canFly //! Static methods are not inherited.
```

This also means constructors are not inherited:

```wren
class Unicorn {
  construct new(name) {
    System.print("My name is " + name + ".")
  }
}

class Pegasus is Unicorn {}

Pegasus.new("Fred") //! Pegasus does not define new().
```

Each class gets to control how it may be constructed independently of its base
classes. However, constructor *initializers* are inherited since those are
instance methods on the new object.

This means you can do `super` calls inside a constructor:

```wren
class Unicorn {
  construct new(name) {
    System.print("My name is " + name + ".")
  }
}

class Pegasus is Unicorn {
  construct new(name) {
    super(name)
  }
}

Pegasus.new("Fred") //> My name is Fred
```

### Super

Sometimes you want to invoke a method on yourself, but using methods defined in
one of your superclasses. You typically do this in
an overridden method when you want to access the original method being
overridden.

To do that, you can use the special `super` keyword as the receiver in a method
call:

```wren
class Base {
  method() {
    System.print("base method")
  }
}

class Derived is Base {
  method() {
    super.method() //> base method
  }
}
```

You can also use `super` without a method name inside a constructor to invoke a
base class constructor:

```wren
class Base {
  construct new(arg) {
    System.print("base got " + arg)
  }
}

class Derived is Base {
  construct new() {
    super("value") //> base got value
  }
}
```

### Attributes

**experimental stage**: subject to minor changes

A class and methods within a class can be tagged with 'meta attributes'.

Like this:

```wren
#hidden = true
class Example {}
```

These attributes are metadata, they give you a way to annotate and store
any additional information about a class, which you can optionally access at runtime.
This information can also be used by external tools, to provide additional
hints and information from code to the tool.

Since this feature has just been introduced, **take note**.

**Currently** there are no attributes with a built-in meaning. 
Attributes are user-defined metadata. This may not remain 
true as some may become well defined through convention or potentially
through use by Wren itself. 

Attributes are placed before a class or method definition,
and use the `#` hash/pound symbol. 

They can be 

- a `#key` on it's own
- a `#key = value`
- a `#group(with, multiple = true, keys = "value")`

An attribute _key_ can only be a `Name`. This is the same type of name 
as a method name, a class name or variable name, an identifier that matches
the Wren identifier rules. A name results in a String value at runtime.

An attribute _value_ can be any of these literal values: `Name, String, Bool, Num`.
Values cannot contain expressions, just a value, there is no compile time 
evaluation.

Groups can span multiple lines, methods have their own attributes, and duplicate
keys are valid.

```wren
#key
#key = value
#group(
  multiple,
  lines = true,
  lines = 0
)
class Example {
  #test(skip = true, iterations = 32)
  doStuff() {}
}
```

#### Accessing attributes at runtime

By default, attributes are compiled out and ignored.

For an attribute to be visible at runtime, mark it for runtime
access using an exclamation:

```wren
#doc = "not runtime data"
#!runtimeAccess = true
#!maxIterations = 16
```

Attributes at runtime are stored on the class. You can access them via 
`YourClass.attributes`. The `attributes` field on a class will 
be null if a class has no attributes or if it's attributes aren't marked.

If the class contains class or method attributes, it will be an object with
two getters:

- `YourClass.attributes.self` for the class attributes
- `YourClass.attributes.methods` for the method attributes

Attributes are stored by group in a regular Wren Map. 
Keys that are not grouped, use `null` as the group key.

Values are stored in a list, since duplicate keys are allowed, multiple
values need to be stored. They're stored in order of definition.

Method attributes are stored in a map by method signature, and each method
has it's own attributes that match the above structure. The method signature
is prefixed by `static` or `foreign static` as needed.

Let's see what that looks like:

```wren
// Example.attributes.self = 
// { 
//   null: { "key":[null] }, 
//   group: { "key":[value, 32, false] }
// }

#!key
#ignored //compiled out
#!group(key=value, key=32, key=false)
class Example {
  #!getter
  getter {}

  // { regular(_,_): { null: { regular:[null] } } }
  #!regular
  regular(arg0, arg1) {}

  // { static other(): { null: { isStatic:[true] } } }
  #!isStatic = true
  static other() {}
  
  // { foreign static example(): { null: { isForeignStatic:[32] } } }
  #!isForeignStatic=32
  foreign static example()
}
```

## Functions

Like many languages today, functions in Wren are little bundles of code 
you can store in a variable, or pass as an argument to a method. 

Notice there's a difference between _function_ and _method_.

Since Wren is object-oriented, most of your code will live in methods on
classes, but free-floating functions are still eminently handy. 

Functions are objects like everything else in Wren, instances of the `Fn`
class.

### Creating a function

To create a function, we call `Fn.new`, which takes a block to execute.
To call the function, we use `.call()` on the function instance.

```wren
var sayHello = Fn.new { System.print("hello") }

sayHello.call() //> hello
```

Note that we'll see a shorthand syntax for creating a function below.

### Function parameters

Of course, functions aren't very useful if you can't pass values to them. The
function above takes no arguments. To change that, you can provide a parameter
list surrounded by `|` immediately after the opening brace of the body.

To pass arguments to the function, pass them to the `call` method:

```wren
var sayMessage = Fn.new {|recipient, message|
  System.print("message for %(recipient): %(message)")
}

sayMessage.call("Bob", "Good day!")
```

It's an error to call a function with fewer arguments than its parameter list
expects. If you pass too *many* arguments, the extras are ignored.

### Returning values

The body of a function is a block. If it is a single
expression—more precisely if there is no newline after the `{` or
parameter list—then the function implicitly returns the value of the
expression.

Otherwise, the body returns `null` by default. You can explicitly return a
value using a `return` statement. In other words, these two functions do the
same thing:

```wren
Fn.new { "return value" }

Fn.new {
  return "return value"
}
```

The return value is handed back to you when using `call`:

```wren
var fn = Fn.new { "some value" }
var result = fn.call()
System.print(result) //> some value
```

### Closures

As you expect, functions are closures—they can access variables defined
outside of their scope. They will hold onto closed-over variables even after
leaving the scope where the function is defined:

```wren
class Counter {
  static create() {
    var i = 0
    return Fn.new { i = i + 1 }
  }
}
```

Here, the `create` method returns the function created on its second line. That
function references a variable `i` declared outside of the function. Even after
the function is returned from `create`, it is still able to read and assign
to`i`:

```wren
var counter = Counter.create()
System.print(counter.call()) //> 1
System.print(counter.call()) //> 2
System.print(counter.call()) //> 3
```

### Callable classes

Because `Fn` is a class, and responds to `call()`, any class can respond to 
`call()` and be used in place of a function. This is particularly handy when 
the function is passed to a method to be called, like a callback or event.

```wren
class Callable {
  construct new() {}
  call(name, version) {
    System.print("called %(name) with version %(version)")
  }
}

var fn = Callable.new()
fn.call("wren", "0.4.0")
```

### Block arguments

Very frequently, functions are passed to methods to be called. There are 
countless examples of this in Wren, like list can be filtered
using a method `where` which accepts a function:

```wren
var list = [1, 2, 3, 4, 5]
var filtered = list.where(Fn.new {|value| value > 3 }) 
System.print(filtered.toList) //> [4, 5]
```

This syntax is a bit less fun to read and write, so Wren implements the 
_block argument_ concept. When a function is being passed to a method, 
and is the last argument to the method, it can use a shorter syntax: 
_just the block part_.

Let's use a block argument for `list.where`, it's the last (only) argument:

```wren
var list = [1, 2, 3, 4, 5]
var filtered = list.where {|value| value > 3 } 
System.print(filtered.toList) //> [4, 5]
```

We've seen this before in a previous page using `map` and `where`:

```wren
numbers.map {|n| n * 2 }.where {|n| n < 100 }
```

### Block argument example

Let's look at a complete example, so we can see both ends.

Here's a fictional class for something that will call a function
when a click event is sent to it. It allows us to pass just a 
function and assume the left mouse button, or to pass a button and a function.

```wren
class Clickable {
  construct new() {
    _fn = null
    _button = 0
  }
  
  onClick(fn) {
    _fn = fn
  }

  onClick(button, fn) {
    _button = button
    _fn = fn
  }

  fireEvent(button) {
    if(_fn && button == _button) {
      _fn.call(button)
    }
  }
}
```

Now that we've got the clickable class, let's use it.
We'll start by using the method that accepts just a function
because we're fine with it just being the default left mouse button.

```wren
var link = Clickable.new()

link.onClick {|button|
  System.print("I was clicked by button %(button)")
}

// send a left mouse click
// normally this would happen from elsewhere

link.fireEvent(0)  //> I was clicked by button 0
```

Now let's try with the extra button argument:

```wren
var contextMenu = Clickable.new()

contextMenu.onClick(1) {|button|
  System.print("I was right-clicked")
}

link.fireEvent(0)  //> (nothing happened)
link.fireEvent(1)  //> I was right-clicked
```

Notice that we still pass the other arguments normally, 
it's only the last argument that is special.

**Just a regular function**   

Block arguments are purely syntax sugar for creating a function and passing it
in one little blob of syntax. These two are equivalent:

```wren
onClick(Fn.new { System.print("clicked") })
onClick { System.print("clicked") }
```

And this is just as valid:

```wren
var onEvent = Fn.new {|button|
  System.print("clicked by button %(button)")
}

onClick(onEvent)
onClick(1, onEvent)
```

**Fn.new**   
As you may have noticed by now, `Fn` accepts a block argument for the `Fn.new`.
All the constructor does is return that argument right back to you!

## Concurrency

Lightweight concurrency is a key feature of Wren and it is expressed using
*fibers*. They control how all code is executed, and take the place of
exceptions in error handling.

Fibers are a bit like threads except they are *cooperatively* scheduled. That
means Wren doesn't pause one fiber and switch to another until you tell it to.
You don't have to worry about context switches at random times and all of the
headaches those cause.

Wren takes care of all of the fibers in the VM, so they don't use OS thread
resources, or require heavyweight context switches. Each just needs a bit of
memory for its stack. A fiber will get garbage collected like any other object
when not referenced any more, so you can create them freely.

They are lightweight enough that you can, for example, have a separate fiber for
each entity in a game. Wren can handle thousands of them without breaking a
sweat. For example, when you run Wren in interactive mode, it creates a new
fiber for every line of code you type in.

### Creating fibers

All Wren code runs within the context of a fiber. When you first start a Wren
script, a main fiber is created for you automatically. You can spawn new fibers
using the Fiber class's constructor:

```wren
var fiber = Fiber.new {
  System.print("This runs in a separate fiber.")
}
```

It takes a function containing the code the fiber should execute. The
function can take zero or one parameter, but no more than that. Creating the
fiber does not immediately run it. It just wraps the function and sits there,
waiting to be activated.

### Invoking fibers

Once you've created a fiber, you run it by calling its `call()` method:

```wren
fiber.call()
```

This suspends the current fiber and executes the called one until it reaches the
end of its body or until it passes control to yet another fiber. If it reaches
the end of its body, it is considered *done*:

```wren
var fiber = Fiber.new {
  System.print("It's alive!")
}

System.print(fiber.isDone) //> false
fiber.call() //> It's alive!
System.print(fiber.isDone) //> true
```

When a called fiber finishes, it automatically passes control *back* to the
fiber that called it. It's a runtime error to try to call a fiber that is
already done.

### Yielding

The main difference between fibers and functions is that a fiber can be
suspended in the middle of its operation and then resumed later. Calling
another fiber is one way to suspend a fiber, but that's more or less the same
as one function calling another.

Things get interesting when a fiber *yields*. A yielded fiber passes control
*back* to the fiber that ran it, but *remembers where it is*. The next time the
fiber is called, it picks up right where it left off and keeps going.

You make a fiber yield by calling the static `yield()` method on Fiber:

```wren
var fiber = Fiber.new {
  System.print("Before yield")
  Fiber.yield()
  System.print("Resumed")
}

System.print("Before call") //> Before call
fiber.call() //> Before yield
System.print("Calling again") //> Calling again
fiber.call() //> Resumed
System.print("All done") //> All done
```

Note that even though this program uses *concurrency*, it is still
*deterministic*. You can reason precisely about what it's doing and aren't at
the mercy of a thread scheduler playing Russian roulette with your code.

### Passing values

Calling and yielding fibers is used for passing control, but it can also pass
*data*. When you call a fiber, you can optionally pass a value to it.

If you create a fiber using a function that takes a parameter, you can pass a
value to it through `call()`:

```wren
var fiber = Fiber.new {|param|
  System.print(param)
}

fiber.call("Here you go") //> Here you go
```

If the fiber has yielded and is waiting to resume, the value you pass to call
becomes the return value of the `yield()` call when it resumes:

```wren
var fiber = Fiber.new {|param|
  System.print(param)
  var result = Fiber.yield()
  System.print(result)
}

fiber.call("First") //> First
fiber.call("Second") //> Second
```

Fibers can also pass values *back* when they yield. If you pass an argument to
`yield()`, that will become the return value of the `call()` that was used to
invoke the fiber:

```wren
var fiber = Fiber.new {
  Fiber.yield("Reply")
}

System.print(fiber.call()) //> Reply
```

This is sort of like how a function call may return a value, except that a fiber
may return a whole sequence of values, one every time it yields.

### Full coroutines

What we've seen so far is very similar to what you can do with languages like
Python and C# that have *generators*. Those let you define a function call that
you can suspend and resume. When using the function, it appears like a sequence
you can iterate over.

Wren's fibers can do that, but they can do much more. Like Lua, they are full
*coroutines*—they can suspend from anywhere in the callstack. The function
you use to create a fiber can call a method that calls another method that calls
some third method which finally calls yield. When that happens, *all* of those
method calls — the entire callstack — gets suspended. For example:

```wren
var fiber = Fiber.new {
  (1..10).each {|i|
    Fiber.yield(i)
  }
}
```

Here, we're calling `yield()` from within a function being
passed to the `each()` method. This works fine in Wren because that inner
`yield()` call will suspend the call to `each()` and the function passed to it
as a callback.

### Transferring control

Fibers have one more trick up their sleeves. When you execute a fiber using
`call()`, the fiber tracks which fiber it will return to when it yields. This
lets you build up a chain of fiber calls that will eventually unwind back to
the main fiber when all of the called ones yield or finish.

This is usually what you want. But if you're doing something low level, like
writing your own scheduler to manage a pool of fibers, you may not want to treat
them explicitly like a stack.

For rare cases like that, fibers also have a `transfer()` method. This switches
execution to the transferred fiber and "forgets" the fiber that was transferred
*from*. The previous one is suspended, leaving it in whatever state it was in.
You can resume the previous fiber by explicitly transferring back to it, or even
calling it. If you don't, execution stops when the last transferred fiber
returns.

Where `call()` and `yield()` are analogous to calling and returning from
functions, `transfer()` works more like an unstructured goto. It lets you freely
switch control between a number of fibers, all of which act as peers to one
another.

## Error Handling

Errors come in a few fun flavors.

### Syntax errors

The first errors you're likely to run into are syntax errors. These include
simple bugs where your code doesn't follow the language's grammar, like:

```wren
1 + * 2
```

Wren detects these errors as soon as it tries to read your code. When it hits
one, you get a friendly error message, like:

    [main line 1] Error on '*': Unexpected token for expression.

Some slightly more "semantic" errors fall into this bucket too. Things like
using a variable that hasn't been defined, or declaring two variables with the
same name in the same scope. So if you do:

```wren
var a = "once"
var a = "twice"
```

Wren tells you:

    [main line 2] Error on 'a': Top-level variable is already defined.

Note that it does this before it executes *any* code. Unlike some other
scripting languages, Wren tries to help you find your errors as soon as
possible when it can.

If it starts running your code, you can be sure you don't have any errors
related to syntax or variable scope.

### Runtime errors

Alas, just fixing all of the "compile-time" errors doesn't mean your code does
what you want. Your program may still have errors that can't be detected
statically. Since they can't be found until your code is run, they're called
"runtime" errors.

Most runtime errors come from the VM itself. They arise from code trying to
perform an operation that the VM can't do. The most common error is a "method
not found" one. If you call a method on an object and its class (and all of its
superclasses) don't define that method, there's nothing Wren can do:

```wren
class Foo {
  construct new() {}
}

var foo = Foo.new()
foo.someRandomMethod
```

If you run this, Wren will print:

    Foo does not implement method 'someRandomMethod'.

Then it stops executing code. Unlike some other languages, Wren doesn't keep
plugging away after a runtime error has occurred. A runtime error implies
there's a bug in your code and it wants to draw your attention to it. To help
you out, it prints a stack trace showing where in the code the error occurred,
and all of the method calls that led to it.

Another common runtime error is passing an argument of the wrong type to a
method. For example, lists are indexed using a number. If you try to pass some
other type, it's an error:

```wren
var list = ["a", "b", "c"]
list["1"]
```

This exits with:

    Subscript must be a number or a range.
    [main line 2] in (script)

These are the two most common kinds of runtime errors, but there are others.
Stuff like out of bounds errors on lists, calling a function with the wrong
number of arguments, etc.

### Handling runtime errors

Most of the time, runtime errors indicate a bug in your code and the best
solution is to fix the bug. However, sometimes it's useful to be able to handle
them at, uh, runtime.

To keep the language simpler, Wren does not have exception handling. Instead, it
takes advantage of fibers for handling errors. When a runtime error occurs,
the current fiber is aborted. Normally, Wren will also abort any fibers that
invoked that one, all the way to the main fiber, and then exit the VM.

However, you can run a fiber using the `try` method. If a runtime error occurs
in the called fiber, the error is captured and the `try` method returns the
error message as a string.

For example, if you run this program:

```wren
var fiber = Fiber.new {
  123.badMethod
}

var error = fiber.try()
System.print("Caught error: " + error)
```

It prints:

    Caught error: Num does not implement method 'badMethod'.

The called fiber can no longer be used, but any other fibers can proceed as
usual. When a fiber has been aborted because of a runtime error, you can also
get the error from the fiber object. Continuing the above example:

```wren
System.print(fiber.error)
```

This also prints:

    Num does not implement method 'badMethod'.

If you have a chain of fiber calls and a runtime error occurs, it will walk the
chain looking for a `try` call, so this can also be used to capture runtime
errors generated in fibers that are invoked by the one you called `try` on.

### Creating runtime errors

Most runtime errors come from within the Wren VM, but you may want to be able
to cause your own runtime errors to occur. This can be done by calling the
`abort()` static method on `Fiber`:

```wren
Fiber.abort("Something bad happened")
```

You must pass in an error message, and it must be a string.

If the provided message is `null`, no runtime error is raised.

### Failures

The last flavor of errors is the highest-level one. All of the above errors
indicate *bugs*—places where the code itself is incorrect. But some
errors indicate that the code simply couldn't accomplish its task for
unforeseeable reasons. We'll call these "failures".

Consider a program that reads in a string of input from the user and parses it
to a number. Many strings are not valid numbers, so this parsing can fail. The
only way the program could prevent that failure is by validating the string
before its parsed, but validating that a string is a number is pretty much the
same thing as parsing it.

For cases like this where failure can occur and the program *will* want to
handle it, fibers and `try()` are too coarse-grained to work with. Instead,
these operations will indicate failure by *returning* some sort of error
indication.

For example, a method for parsing a number could return a number on success and
`null` to indicate parsing failed. Since Wren is dynamically typed, it's easy
and natural for a method to return different types of values.

## Modularity

Once you start writing programs that are more than little toys, you quickly run
into two problems:

1. You want to break them down into multiple smaller files to make it easier to
   find your way around them.

2. You want to reuse pieces of them across different programs.

To address those, Wren has a simple module system. A file containing Wren code
defines a *module*. A module can use the code defined in another module by
*importing* it. You can break big programs into smaller modules that you
import, and you can reuse code by having multiple programs share the use of a
single module.

Wren does not have a single global scope. Instead, each module has its own
top-level scope independent of all other modules. This means, for example, that
two modules can define a top-level variable with the same name without causing
a name collision. Each module is, well, modular.

### Importing, briefly

When you run Wren and give it a file name to execute, the contents of that file
define the "main" module that execution starts at. To load and execute other
modules, you use an import statement:

```wren
import "beverages" for Coffee, Tea
```

This finds a module named "beverages" and executes its source code. Then, it
looks up two top-level variables, `Coffee` and `Tea` in *that* module and
creates new variables in *this* module with their values.

This statement can appear anywhere a variable declaration is allowed, even
inside blocks:

```wren
if (thirsty) {
  import "beverages" for Coffee, Tea
}
```

If you need to import a variable under a different name, you can use 
`import "..." for Name as OtherName`. This looks up the top-level variable
`Name` in *that* module, but declares a variable called `OtherName` in *this* module
with its value.

```wren
import "liquids" for Water //Water is now taken
import "beverages" for Coffee, Water as H2O, Tea
// var water = H2O.new()
```

If you want to load a module, but not bind any variables from it, you can omit
the `for` clause:

```wren
import "some_imperative_code"
```

That's the basic idea. Now let's break it down into each of the steps it
performs:

1. Locate the source code for the module.
2. Execute the imported module's code.
3. Bind new variables in the importing module to values defined in the imported
   module.

We'll go through each step:

### Locating a module

The first thing you need to do to import a module is actually *find* the code
for it. The import specifies a *name*—some arbitrary string that is used
to uniquely identify the module. The embedding application controls how that
string is used to locate a blob of source code.

When the host application creates a new Wren VM, it provides a module loader
function:

WrenConfiguration config;
config.loadModuleFn = loadModule;

// Other configuration...

WrenVM* vm = wrenNewVM(&config);

That function has this signature:

WrenLoadModuleResult WrenLoadModuleFn(WrenVM* vm, const char* name);

Whenever a module is imported, the VM calls this and passes it the name of the
module. The embedder is expected to return the source code contents of the
module in a `WrenLoadModuleResult`. When you embed Wren in your app, you can handle
this however you want: reach out to the file system, look inside resources bundled
into your app, whatever.

You can return the source field as `NULL` from this function to indicate that a module
couldn't be found. When you do this, Wren will report it as a runtime error.

#### The command-line loader

The Wren CLI command-line tool has a very simple
lookup process. It appends the module name and ".wren" to the directory where
the main module was loaded and looks for that file. So, let's say you run:

    $ wren code/my_program.wren

And that main module has:

```wren
import "some/module"
```

Then the command-line VM will try to find `/code/some/module.wren`. By
convention, forward slashes should be used as path separators, even on Windows,
to help ensure your scripts are platform-independent. (Forward slashes are a
valid separator on Windows, but backslashes are not valid on other OSes.)

### Executing the module

Once we have the source code for a module, we need to run it. First, the VM
takes the fiber that is executing the `import` statement in the importing
module and pauses it.

Then it creates a new module object—a new fresh top-level scope,
basically—and a new fiber. It executes the new module's code in that
fiber and scope. The module can run whatever imperative code it wants and
define whatever top-level variables it wants.

When the module's code is done being executed and its fiber completes, the
suspended fiber for the importing module is resumed. This suspending and
resuming is recursive. So, if "a" imports "b" which imports "c", both "a" and
"b" will be suspended while "c" is running. When "c" is done, "b" is resumed.
Then, when "b" completes, "a" is resumed.

Think of it like traversing the tree of imports, one node at a time. At any
given point in time, only one module's code is running.

### Binding variables

Once the module is done executing, the last step is to actually *import* some
data from it. Any module can define "top-level" variables.
These are simply variables declared outside of any
method or function.

These are visible to anything inside the module, but they can also be
*exported* and used by other modules. When Wren executes an import like:

```wren
import "beverages" for Coffee, Tea
```

First it runs the "beverages" module. Then it goes through each of the variable
names in the `for` clause. For each one, it looks for a top-level variable with
that name in the imported module. If a variable with that name can't be found
in the imported module, it's a runtime error.

Otherwise, it gets the current value of the variable and defines a new variable
in the importing module with the same name and value. It's worth noting that
the importing module gets its *own* variable whose value is a snapshot of the
value of the imported variable at the time it was imported. If either module
later assigns to that variable, the other won't see it. It's not a "live"
connection.

In practice, most top-level variables are only assigned once anyway, so this
rarely makes a difference.

### Shared imports

Earlier, I described a program's set of modules as a tree. Of course, it's only
a *tree* of modules if there are no *shared imports*. But consider a program
like:

```wren
// main.wren
import "a"
import "b"

// a.wren
import "shared"

// b.wren
import "shared"

// shared.wren
System.print("Shared!")
```

Here, "a" and "b" both want to use "shared". If "shared" defines some top-level
state, we only want a single copy of that in memory. To handle this, a module's
code is only executed the *first* time it is loaded. After that, importing the
module again just looks up the previously loaded module.

Internally, Wren maintains a map of every module it has previously loaded. When
a module is imported, Wren looks for it in that map first before it calls out
to the embedder for its source.

In other words, in that list of steps above, there's an implicit zeroth step:
"See if we already loaded the module and reuse it if we did". That means the
above program only prints "Shared!" once.

### Cyclic imports

You can even have cycles in your imports, provided you're a bit careful with
them. The loading process, in detail, is:

1. See if we have already created a module with the given name.
2. If so, use it.
3. Otherwise, create a new module with the name and store it in the module
   registry.
4. Create a fiber for it and execute its code.

Note the order of the last two steps. When a module is loaded, it is added to
the registry *before* it is executed. This means if an import for that same
module is reached while the module itself or one of its imports is executing,
it will be found in the registry and the cycle is short-circuited.

For example:

```wren
// main.wren
import "a"

// a.wren
System.print("start a")
import "b"
System.print("end a")

// b.wren
System.print("start b")
import "a"
System.print("end b")
```

This program runs successfully and prints:

    start a
    start b
    end b
    end a

Where you have to be careful is binding variables. Consider:

```wren
// main.wren
import "a"

// a.wren
import "b" for B
var A = "a variable"

// b.wren
import "a" for A
var B = "b variable"
```

The import of "a" in b.wren will fail here. If you trace the execution, you
get:

1. Execute `import "a"` in "main.wren". That suspends "main.wren".
2. Execute `import "b"` in "a.wren". That suspends "a.wren".
3. Execute `import "a"` in "b.wren". Since "a" is already in the module map,
   this does *not* suspend it.

Instead, we look for a variable named `A` in that module. But it hasn't been
defined yet since "a.wren" is still sitting on the `import "b" for B` line
before the declaration. To get this to work, you would need to move the
variable declaration above the import:

```wren
// main.wren
import "a"

// a.wren
var A = "a variable"
import "b" for B

// b.wren
import "a" for A
var B = "b variable"
```

Now when we run it, we get:

1. Execute `import "a"` in "main.wren". That suspends "main.wren".
2. Define `A` in "a.wren".
2. Execute `import "b"` in "a.wren". That suspends "a.wren".
3. Execute `import "a"` in "b.wren". Since "a" is already in the module map,
   this does *not* suspend it. It looks up `A`, which has already been defined,
   and binds it.
4. Define `B` in "b.wren".
5. Complete "b.wren".
6. Look up `B` in "b.wren" and bind it in "a.wren".
7. Resume "a.wren".

This sounds super hairy, but that's because cyclic dependencies are hairy in
general. The key point here is that Wren *can* handle them in the rare cases
where you need them.

### Exiting a module early

Although the `return` statement is normally used to exit from a method or a function, it can also be used from a module's top-level code to exit the module. For example, if the script consists of a single module, this code would exit the module (and therefore the script) early:

```wren
for (i in 1..2) {
  if (i == 2) return
  System.print(i)  //>  prints 1 but not 2
}
System.print(3)    //>  not reached
```

Although it is not invalid to return a value, there is no way to access that value and it is therefore simply discarded.
