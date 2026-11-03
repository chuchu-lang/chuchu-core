# Records

A record groups named fields into one value. Declare the type once at the top
level, then create values with an initializer and read and write their fields with
`.`.

```chuchu
record Address {
    city: string
    zip: int
}

record Student {
    name: string
    age: int
    gpa: float
    active: bool
    address: Address
}

assign student as Student {
    name: "Ali"
    age: 22
    gpa: 3.5
    active: true
    address: Address {
        city: "Lahore"
        zip: 54000
    }
}

show student.name
show student.address.city
reassign student.age as 25
reassign student.address.city as "Karachi"
```

The [runnable example](examples/records.chuchu) shows copies and function calls.

## Declaring records

```chuchu
record Name {
    field: type
    another: type
}
```

* Declare records at the top level; not inside functions or blocks. A record may
  be used before the line that declares it.
* Put one field per line. A record needs at least one field, and field names must
  be different within one record.
* A field is `int`, `float`, `bool`, `string`, or another record. A record cannot
  contain itself, directly or through other records, because it is stored by
  value: `record Node { next: Node }` is an error.
* Record names cannot be `int`, `float`, `bool`, `string` or `result`.

## Creating records

`Name { field: value ... }` creates a record. Every field must be given exactly
once, in any order. Values are evaluated in the order they are written and stored
in the order the record declares them. An `int` fills a `float` field, as it does
everywhere else. Write one field per line; a single line is fine when it holds one
field: `Point { x: 1 }`.

An initializer is an expression: it can be assigned, passed to a function,
returned, used as a field of another initializer, or read directly:
`show Point { x: 3 }.x`. In an `if` or `while` condition, wrap an initializer in
parentheses so the `{` is not mistaken for the start of the block.

## Reading and writing fields

`variable.field` reads a field and `variable.a.b` reads through nested records.
A field can also be read from a function result, `make().name`, or an initializer.

`reassign variable.field as value` replaces one field and leaves the others as
they were. It works through nested records, `reassign s.address.city as "Lahore"`,
and the value must have the field's type (an `int` is accepted for a `float`
field). Only a variable can be written through; the target cannot be a function
result, a record initializer, or an array element.

## Copies have value semantics

Records are values, not references. Assigning, reassigning, passing or returning a
record gives an independent copy, including every string and nested record inside
it. Changing one never changes the other.

```chuchu
assign a as student
assign b as a
reassign b.name as "Bo"
reassign b.address.city as "Quetta"
show a.name              // still Ali
```

`reassign a as b` replaces the whole value of `a` with a copy of `b`, so
`reassign a as a` is safe and does nothing. Records cannot be compared with `==`,
put in arrays, or printed whole with `show`; print their fields.

## Ownership and memory

A string is a heap allocation owned by exactly one place. A record owns the
strings in its fields, including those of records nested inside it, so copying a
record copies those strings and releasing a record releases them. The compiler
releases them for you:

* when a variable goes out of scope, at the end of its block, function or program,
  and on `return`;
* when a variable or field is assigned a new value, the old value is released
  first;
* when a temporary, such as `make().name` or an initializer that is only read from,
  has been used.

Records made only of `int`, `float` and `bool` fields own nothing and are plain
values. Nothing is shared between copies, so there is no way to create a dangling
reference or to free a value twice.

## Functions

Parameters and return types may name a record. Arguments are passed by value: a
function that changes its parameter changes its own copy.

```chuchu
function birthday(student: Student) be Student {
    reassign student.age as student.age + 1
    return student
}

assign older as birthday(student)   // student itself is unchanged
```

A function can return a record it built, with its strings, and the result stays
valid after the function exits. As with all functions, a function cannot see
variables declared outside it; pass a record in as an argument.

## Errors

The compiler checks records before generating any code, and reports:

| Problem | Example |
| --- | --- |
| Unknown record | `assign s as Nope { x: 1 }`, `function f(p: Nope) ...` |
| Unknown or repeated field | `Student { agee: 1 }`, a field given twice |
| Missing field | `Student { name: "Ali" }` without the other fields |
| Wrong type | `student.age` assigned `"old"`, `Student { age: "x" }` |
| Reading or writing a field of a non-record | `student.name.length` |
| Recursive record | `record A { b: B }` with `record B { a: A }` |
| Invalid assignment target | `reassign make().age as 1` |
| Printing a whole record | `show student` |

## Limitations

* No arrays of records, record equality, or printing a whole record.
* No methods, inheritance, or references: records are plain values.
* Records cannot be declared inside functions or blocks.
* Copying is a deep copy of every string, so very large copies in a hot loop
  cost time; pass only what a function needs.
