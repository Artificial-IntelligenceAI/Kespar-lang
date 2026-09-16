# oracle — the Kespar reference interpreter

A deliberately slow, small and obvious tree-walking interpreter for Kespar,
written from `design/language.md` alone (no code or ideas shared with
`kespar/`). It has no Precog: every check site (§8.4) is checked at run
time, every time, in every tier; free `:=` names (§8.3 step 2) are
unbounded integers (`i128`) or `bin64`.

```
cargo build
cargo test

oracle [--steps N] [--trace-checks] file.kpls < input
oracle --types file.kpls
```

- `--steps N` — a step budget (see "Steps" below). On exhaustion:
  `kespar: step budget exhausted` on stderr, exit 4.
- `--trace-checks` — after the run (whatever its outcome), one line per
  check site in source order, `site <line> <kind> fired|clean`, then one
  line per free integer name, `name '<n>' min <v> max <v>` or
  `name '<n>' unreached`, on stdout after the program's own output.
- `--types` — the §8.3 step-1 forcing result per `:=` name, in
  declaration order: `'<n>' int32` / `'<n>' free`.

Exit codes are §9's: 0, 1 (`kespar: <kind> at line N`), 2
(`kespar: bad input for 'n' at line N`), 3 (`error: … at line N`), `n`
for `std::exit[n]`, plus 4 for the step budget and **5** for an oracle
limit (a free integer leaving the `i128` range — see below).

## Steps

A step (§8.5, oracle.md "What the oracle never does") is one statement
executed, one expression node evaluated, one loop iteration or one read,
**plus one step per 64 bytes of text or list built**: every time text is
produced (pieces joined into a str, `std::to.str.utf8`, the rendered text
of a `print`, a str read) or a list is created (a list literal,
`std::fill`, a list read) the interpreter charges `bytes / 64` extra
steps, where `bytes` is the text's UTF-8 length or the list's element
count × 16. So a loop that builds a string quadratically exhausts the
budget instead of memory. A `std::fill` is charged before its list is
allocated. The message and exit code are the same as for any other
exhaustion: `kespar: step budget exhausted`, exit 4. Strings are
immutable `Rc<str>` values, rebuilt on every concatenation and dropped
when the name is reassigned, so memory does not grow with the number of
iterations of such a loop (`tests/cases/step_budget_text.kpls`).

## Layout

- `src/lexer.rs` — §1.
- `src/parser.rs` — §1, §3, §5, §6.1, §6.2 (a Pratt parser plus the
  "parenthesise" adjacency rules), §7.
- `src/check.rs` — types, no implicit conversion, `[ ]` rules,
  shadowing, immut, arity, `MAIN`, the boundary rule, §8.3 step-1 forcing
  (union-find over type variables), literal fitting, the §6.6 table, and
  the check-site list in source order.
- `src/interp.rs` — §4 reads, §5, §6.3–6.6, §8.4 checks, §9.
- `src/render.rs` — §6.7 stored/useful rendering.
- `src/lib.rs` — `compile`, `run`, `types_listing` for a harness.
- `tests/cases/*.kpls` with `.out` (stdout), `.exit` (default 0), `.in`
  (stdin), `.err` (stderr prefix), `.args` (extra flags); run by
  `tests/programs.rs` through the built binary.

## Ambiguities found in the spec

Where `design/language.md` was silent or ambiguous the most literal
reading was implemented. Each is listed with the section and the choice.

1. **§1 operator words as function names.** `x xx mod and or not` are not
   reserved. Chosen: in operator position `x xx mod and or` are always
   operators; in operand position a bare word followed by `[` is a call
   (so `x[1, 2]` calls a function named `x`), except `not`, which is
   always the unary operator (a function named `not` cannot be called).
2. **§1 "`-5` is negation applied to `5`".** Taken literally: the literal
   is `5`, so `var.int8 'x' = [-128]` is a compile error (`128` does not
   fit `int8`); `int8`'s minimum is reached by computation or by a read.
   Read bounds (§4) do accept a leading `-` because the spec's own example
   writes `-50` there.
3. **§1 integer literals** are limited to `i128`; a larger literal is
   rejected with `integer literal too large`. A quoted name may not be
   empty.
4. **§3 `$:=` / `$=`** may hide a name declared in the same block, not
   only in an enclosing one ("a visible name").
5. **§3 element assignment on an `immut` list** (`'xs'[i] = […]`) is
   allowed: the rule forbids assigning to the *name*.
6. **§3/§5 `loop.for` variable** declared while a same-named name is
   visible is a compile error (there is no `$` form for it).
7. **§4 list reads without a bound** (`std::read.stdin[]` into a
   `list.int16`) are allowed, any length. Length bounds of a list or str
   read are any non-negative integer literals (they are counts, not
   values "of the declared type").
8. **§4 typed assignment.** The target of `'n' = [std::read.stdin[…]]`
   must be an explicitly typed name (`var.T`); a `:=` name, even one
   already forced, gets the boundary-rule error. The read's error line is
   the line of the `std::read` token.
9. **§5 print.** `std::print.stdout[]` (no pieces) and a comma inside a
   print bracket are compile errors. A parenthesised formula piece
   (`["a" ('x' + 1)]`) or `['n' - 1]` is allowed; only a bare numeric
   literal is "a bare number".
10. **§6.1 `[[1], [2]]`** is a list of two *numbers*, because `[1]` is the
    number 1; a one-element list is `[1,]`, so a list of one-element
    lists is `[[1,], [2,]]`. A trailing comma inside an *argument* list
    (`f[1,]`) is a compile error.
11. **§6.2 `mod` beside any operator** includes unary `-`: `-7 mod 2` is a
    compile error, write `(-7) mod 2`. The operand of `xx` may not be an
    unparenthesised unary minus (`2 xx -1` is an error; `2 xx (-1)`).
    `not` beside a comparison in either direction is an error. Messages
    read `parenthesise: '<a>' beside '<b>'`.
12. **§6.5 `std::len`** whose result does not fit the integer type its
    context needs (`var.int8 'n' = [std::len['xs']]` with 200 elements):
    treated as an **overflow** check site at the `std::len`.
13. **§7 untyped functions with a `return [v]`** must return on every
    path, like typed ones. A `loop { }` with no `break` of its own, and
    `std::exit`, count as not falling through. A function that returns
    nothing may be called only as a statement.
14. **§7 namespaces.** Function names (bare) and variable names (quoted)
    never clash.
15. **§8.3 the family of a `:=` name.** A name whose initial expression is
    an integer-looking literal (or is built only from such literals, or
    from `std::len`) has the *integer* family: `var 'x' := [1];
    'x' = [1.5];` is a compile error. An untyped parameter met only by
    integer literals stays open to both families until something decides
    (`f[1]; f[2.5];` makes it a bin and the `1` a bin).
16. **§8.3 comparisons.** The *operands* of `== !== < > <== >==` meet each
    other and force; only the bool result forces nothing. `and`/`or`/
    `not` operands are `bool`.
17. **§8.3 literal-only expressions with no context** (`if [(2 xx 100) ==
    0]`) are free: unbounded, never overflowing.
18. **§8.3 `--types`.** A name is `free` when any leaf of its type was
    forced by nothing (free integers, free bins, lists of them);
    otherwise its full type is printed (`bool`, `str.utf8`, `list.int32`
    …). The return value of an untyped function is listed as
    `'<fname>()'`. Loop counters/elements and untyped parameters are
    listed at their declaration line.
19. **§8.4 / oracle.md trace.** Sites typed by a free name are listed too
    (always `clean`, since they cannot fail). `xx` has two sites at one
    line (negative exponent, then overflow), `/` and `mod` two (division
    by zero, then overflow); `std::to.T` to the very same type has none.
    The trace is printed after every outcome, including a failure or
    `std::exit`. Free-name `min`/`max` cover every value of every
    expression whose type the name decides (§8.3 step 2), e.g. the
    bounds of a `std::range` even when the loop runs zero times.
20. **§8.4 `check { }`** cannot produce "cannot prove" compile errors
    without Precog; it behaves as the default tier (checked at run time).
21. **§8.3 step 2 / §9 oracle limit.** A free integer is an `i128`; a
    program that pushes one past `i128` stops with exit **5** and
    `kespar: oracle limit: a free integer left the i128 range at line N`
    (Precog would have refused such a program at compile time).
22. **§9 one error at a time.** Only the first compile error is reported;
    forcing/type errors (pass 1) are found before literal-fit and
    `std::to`-table errors (pass 2), even when the latter come earlier in
    the file.
23. **§4 str reads** of non-UTF-8 bytes are decoded lossily (the spec
    says files are UTF-8 and says nothing about input).
