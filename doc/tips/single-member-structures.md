
## Prefer simple members to one-member structures

** This is a proposed guideline for new API development,
_not_ a change to existing apis.
Implementing this guideline for some of the existing actor interfaces,
such as keyvalue, would be a breaking change, and is out of scope
for this RFC.**

A function is modeled as an operation, with an optional `input` type 
for its parameters, and an optional `output` type for its return value.
If an input or output type is a structure containing one member, it
is preferred to simplify the declaration and api to use the member,
instead of the structure. For example, instead of 

```text
/// count the number of values matching a query string
operation Count {
    input: String,
    output: CountResponse,
}

structure CountResponse {
    value: U32,
}
```

Use
```text
/// count the number of values matching a query string
operation Count {
    input: String,
    output: U32,
}
```

One reason you might still prefer to use a structure in this case
is if the api is expected to require (as inputs) or return (as output)
additional structure members. 
Using a structure, even with one member, may reduce the amount
of code that breaks when the structure is updated later to add new
members. For future compatibility with existing
code, new structure members should generally be added as optional fields.

As a general rule, I propose that unless there is a compelling example of a
need for additional values to be added later, for an input or output structure,
that structures with a single member be eliminated from the api, 
and the input or output value changed to the simple type.



