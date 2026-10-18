# List

Extends Sequence.

An indexable contiguous collection of elements. More details here.

## Static Methods

### List.**filled**(size, element)

Creates a new list with `size` elements, all set to `element`.

It is a runtime error if `size` is not a non-negative integer.

### List.**new**()

Creates a new empty list. Equivalent to `[]`.

## Methods

### **add**(item)

Appends `item` to the end of the list. Returns the added item.

### **addAll**(other)

Appends each element of `other` in the same order to the end of the list. `other` must be an iterable.

```wren
var list = [0, 1, 2, 3, 4]
list.addAll([5, 6])
System.print(list) //> [0, 1, 2, 3, 4, 5, 6]
```

Returns the added items.

### **clear**()

Removes all elements from the list.

### **count**

The number of elements in the list.

### **indexOf**(value)

Returns the index of `value` in the list, if found. If not found, returns -1.

```wren
var list = [0, 1, 2, 3, 4]
System.print(list.indexOf(3)) //> 3
System.print(list.indexOf(20)) //> -1
```

### **insert**(index, item)

Inserts the `item` at `index` in the list.

```wren
var list = ["a", "b", "c", "d"]
list.insert(1, "e")
System.print(list) //> [a, e, b, c, d]
```

The `index` may be one past the last index in the list to append an element.

```wren
var list = ["a", "b", "c"]
list.insert(3, "d")
System.print(list) //> [a, b, c, d]
```

If `index` is negative, it counts backwards from the end of the list. It bases this on the length of the list *after* inserted the element, so that `-1` will append the element, not insert it before the last element.

```wren
var list = ["a", "b"]
list.insert(-1, "d")
list.insert(-2, "c")
System.print(list) //> [a, b, c, d]
```

Returns the inserted item.

```wren
System.print(["a", "c"].insert(1, "b")) //> b
```

It is a runtime error if the index is not an integer or is out of bounds.

### **iterate**(iterator), **iteratorValue**(iterator)

Implements the iterator protocol for iterating over the elements in the
list.

### **remove**(value)

Removes the first value found in the list that matches the given `value`, 
using regular equality to compare them. All trailing elements
are shifted up to fill in where the removed element was.

```wren
var list = ["a", "b", "c", "d"]
list.remove("b")
System.print(list) //> [a, c, d]
```

Returns the removed value, if found.
If the value is not found in the list, returns null.

```wren
System.print(["a", "b", "c"].remove("b")) //> b
System.print(["a", "b", "c"].remove("not found")) //> null
```

### **removeAt**(index)

Removes the element at `index`. If `index` is negative, it counts backwards
from the end of the list where `-1` is the last element. All trailing elements
are shifted up to fill in where the removed element was.

```wren
var list = ["a", "b", "c", "d"]
list.removeAt(1)
System.print(list) //> [a, c, d]
```

Returns the removed item.

```wren
System.print(["a", "b", "c"].removeAt(1)) //> b
```

It is a runtime error if the index is not an integer or is out of bounds.

### **sort**(), **sort**(comparer)

Sorts the elements of a list in-place; altering the list. The default sort is implemented using the quicksort algorithm.

```wren
var list = [4, 1, 3, 2].sort()
System.print(list) //> [1, 2, 3, 4]
```

A comparison function `comparer` can be provided to customise the element sorting. The comparison function must return a boolean value specifying the order in which elements should appear in the list.

The comparison function accepts two arguments `a` and `b`, two values to compare, and must return a boolean indicating the inequality between the arguments. If the function returns true, the first argument `a` will appear before the second `b` in the sorted results.

A compare function like `{|a, b| true }` will always put `a` before `b`. The default compare function is `{|a, b| a < b }`.

```wren
var list = [9, 6, 8, 7]
list.sort {|a, b| a < b}
System.print(list) //> [6, 7, 8, 9]
```

It is a runtime error if `comparer` is not a function.

### **swap**(index0, index1)

Swaps values inside the list around. Puts the value from `index0` in `index1`,
and the value from `index1` at `index0` in the list.

```wren
var list = [0, 1, 2, 3, 4]
list.swap(0, 3)
System.print(list) //> [3, 1, 2, 0, 4]
```

### **[**index**]** operator

Gets the element at `index`. If `index` is negative, it counts backwards from
the end of the list where `-1` is the last element.

```wren
var list = ["a", "b", "c"]
System.print(list[1]) //> b
```

If `index` is a Range, a new list is populated from the elements
in the range.

```wren
var list = ["a", "b", "c"]
System.print(list[0..1]) //> [a, b]
```

You can use `list[0..-1]` to shallow-copy a list.

It is a runtime error if the index is not an integer or range, or is out of bounds.

### **[**index**]=**(item) operator

Replaces the element at `index` with `item`. If `index` is negative, it counts
backwards from the end of the list where `-1` is the last element.

```wren
var list = ["a", "b", "c"]
list[1] = "new"
System.print(list) //> [a, new, c]
```

It is a runtime error if the index is not an integer or is out of bounds.

### **+**(other) operator

 Appends a list to the end of the list (concatenation). `other` must be an iterable.

```wren
var letters = ["a", "b", "c"]
var other = ["d", "e", "f"]
var combined = letters + other
System.print(combined)  //> [a, b, c, d, e, f]
```

### **\***(count) operator

Creates a new list by repeating this one ```count``` times. It is a runtime error if ```count``` is not a non-negative integer.

```wren
var digits = [1, 2]
var tripleDigits = digits * 3
System.print(tripleDigits) //> [1, 2, 1, 2, 1, 2] 
```
