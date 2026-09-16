# The Kespar language

Kespar is a small language that exists to prove Cyborg's Precog. Its syntax
is **CyborgPL's** (owner, 2026-09-16: "just copy Cyborg's syntax, don't
wanna design another lang for a quick POC"), cut down to the POC's scope
(owner: int, uint, bin, bool, str, if/else, func, print, input, arr, GC,
loops). Where CyborgPL has decided something, Kespar copies it; where
CyborgPL has left it open, Kespar makes a pick in CyborgPL's style and marks
it **provisional**. Nothing provisional is a decision, for Kespar or for
CyborgPL. Source for the copied parts: `/Users/ts/CyborgPL v2/design/*.md`
and its `DECISIONS.md` as of 2026-09-16.

This file is the complete specification. The reference interpreter in
`oracle/` is written from this file alone; the compiler in `kespar/` must
agree with it. Where the two disagree, this file decides which is wrong.

Source files end in **`.kpls`** (owner, 2026-09-16). Encoding is UTF-8.

## 1. Lexical structure

- **Comments**: `//` to end of line (**provisional** — CyborgPL has not
  designed comments; its docs use `//`).
- **Names are quoted**: `'total'`, `'xs'`, `'my name'`. Anything except
  `'` and a newline may appear inside. A quoted name is always a variable.
- **Function names are bare** identifiers `[A-Za-z_][A-Za-z0-9_]*`, not a
  keyword.
- **Keywords** (bare, reserved): `var` `func` `return` `if` `else` `loop`
  `in` `break` `continue` `check` `nocheck` `true` `false` `mut` `immut`
  `MAIN`, and the chain words `int8 int16 int32 int64 uint8 uint16 uint32
  uint64 bin32 bin64 bool str utf8 list while for`. The operator words `x
  xx mod and or not` are not reserved (CyborgPL: an operator sits between
  operands, a call is a word followed by `[`).
- **`std::`** reaches the standard Limb. Kespar has exactly one other Limb
  than the program: `std`. There is no Limb declaration.
- **Integer literals**: decimal digits, `_` allowed between digits
  (`1_000_000`). No sign — `-5` is negation applied to `5`.
- **Bin literals**: digits `.` digits, optional exponent (`1.5`,
  `2.0e10`). `1.` and `.5` are errors.
- **Text literals**: `"..."` with escapes `\\` `\"` only (**provisional**);
  a newline in text is the `\n` *piece* (§5), never an escape inside the
  quotes. Raw newlines may not appear inside.
- **Bool literals**: `true`, `false`.
- **Statements end with `;`**; blocks are `{ }` and take no `;`.
- Three brackets: `[ ]` value slot(s), `{ }` body, `( )` arithmetic
  grouping only.

## 2. Types

Spelled as chain segments (`var.int32`, `var.list.int16`).

| Chain | Meaning |
|---|---|
| `int8` `int16` `int32` `int64` | two's-complement signed integer |
| `uint8` `uint16` `uint32` `uint64` | unsigned integer |
| `bin32` `bin64` | IEEE 754 binary32 / binary64 |
| `bool` | `true` / `false` |
| `str.utf8` | text; the encoding segment is mandatory and `utf8` is the only one Kespar has (**provisional** cut of CyborgPL's `utf16/utf32/ascii`) |
| `list.T` | fixed-length mutable list of `T`, any `T` including `list.U` (**provisional** — CyborgPL has not named its list type) |

Not in Kespar: `bin16/128`, `deci`, `int128/uint128`, `char`, `std::uer`.

**The typed form is always fully explicit** (CyborgPL, decided): `var.int32`
never `var.int`, `var.str.utf8` never `var.str`. **No implicit conversion**
between any two types, not even `int32` → `int64`; mixing is a compile
error and `std::to` converts (§6.6). **A bare literal adopts its context's
family and type**: `var.int8 'x' = [5];` makes `5` an `int8`;
`var.bin64 'x' = [1];` makes `1` a `bin64`; `['x' + 1]` with `'x'` a
`uint64` makes `1` a `uint64`. A literal that does not fit its context type
is a compile error (`var.int8 'x' = [300];`).

**Memory is garbage collected** (owner, 2026-09-16). CyborgPL's ownership
words (`give` `copy` `lend` `lend.mut` `lent`) **do not exist** in Kespar:
a `str` or `list` is handed around freely. A `list` is a reference —
`'b' = ['a'];` makes both names the same list and writes through one show
through the other. `str` is immutable, so sharing is unobservable.

## 3. Declarations, `:=` and `=`

```
var.int32 'n' = [10];          // explicit type, plain =
var 'total' := [0];            // no type: Precog picks the best type (§8.3)
'total' = ['total' + 1];       // assignment; no += (CyborgPL, decided)
var.immut.int32 'limit' = [10];   // assigning to 'limit' later is an error
var.mut 'counter' := [0];         // statable, enforces nothing
```

- **Typed**: `var.T 'name' = [expr];`. **Inferred**: `var 'name' := [expr];`.
  `:=` appears **only** in the untyped form and `=` only in the typed form
  (CyborgPL, decided). A `:=` whose value cannot be bounded is a **compile
  error** (§8.3), never a silent wide type.
- **`[ ]` is mandatory in every value position** (CyborgPL, decided): a
  lone literal is still `[1000]`, a call's result is `[add[1, 2]]`.
- **Assignment** `'name' = [expr];` to a visible name of exactly that type.
  `immut` names cannot be assigned (compile error). Assigning to a
  `loop.for` variable is a compile error.
- **Element assignment**: `'xs'[i] = [expr];` (**provisional** index shape,
  from CyborgPL's `'items'[3]` example).
- **Scope**: a block is a scope; a name lives to its end. **Shadowing is
  never silent** (CyborgPL, decided): redeclaring a visible name is a
  compile error unless written `$:=` / `$=`, which hides it until the block
  ends; `$` with nothing to hide is an error; an `immut` name cannot be
  shadowed.
- Every declaration has a value; there are no uninitialised names.

## 4. Input — the boundary

CyborgPL has not designed input (open). Kespar's **provisional** pick, in
chain style: `std::read.stdin` is a value that takes its type from its
context, exactly like a literal, and carries its bound as its arguments.

```
var.int32 'n' = [std::read.stdin[]];                 // any int32
var.int32 'n' = [std::read.stdin[0, 1000]];          // 0 ≤ n ≤ 1000
var.uint8 'k' = [std::read.stdin[1, 8]];
var.bin64 'x' = [std::read.stdin[]];                 // bins take no bound
var.bool 'flag' = [std::read.stdin[]];
var.str.utf8 's' = [std::read.stdin[]];              // one line
var.str.utf8 's' = [std::read.stdin[1, 80]];         // 1 ≤ scalar count ≤ 80
var.list.int16 'xs' = [std::read.stdin[1, 100]];     // 1 ≤ length ≤ 100
var.list.int16 'xs' = [std::read.stdin[1, 100, -50, 50]];  // …and each in -50..50
```

- **The boundary rule** (CyborgPL, decided): the value from outside must
  carry an explicit type. `var 'n' := [std::read.stdin[]];` is a compile
  error: *"'n' comes from outside the program; write its type"*.
  `std::read.stdin` may appear only as the whole value of a typed `var`
  declaration or typed assignment (**provisional**).
- Bounds are literals of the declared type, inclusive both ends, `lo <= hi`.
  For a scalar integer: `[lo, hi]`. For a `str`: `[lo, hi]` bounds the
  scalar-value count, non-negative. For a `list.T`: `[lo, hi]` bounds the
  length; `[lo, hi, elo, ehi]` also bounds every element (integer `T`
  only). `bool`, bins and `list.list` take no bound; `list.list` and
  `list.str.utf8` and `list.bool` and `list.bin*` cannot be read at all —
  only `list` of an integer type (**provisional**).
- Reads may appear anywhere a statement may, in program order.

**Reading** (**provisional**): standard input, **one line per read**. A
scalar reads its line trimmed of ASCII whitespace. A list reads its
elements whitespace-separated on that line (an empty line is length 0). A
str reads the line verbatim without its `\n` (a trailing `\r` is dropped
too). Integer tokens: optional `-` and decimal digits, no `_`. Bin tokens:
what Rust's `f64::from_str` / `f32::from_str` accept (`1`, `-2.5`, `1e3`,
`inf`, `NaN`). Bool tokens: exactly `true` / `false`.

**Contract violation**: a missing line, a malformed token, a value that
does not fit its type, or a value/length outside its bound stops the
program with exit code **2** and

```
kespar: bad input for 'n' at line 12
```

on stderr (the source line of the read). This check is **always** made at
run time and Precog never removes it — it is the guardrail every proof
downstream stands on.

## 5. Statements

```
std::print.stdout['total' \n];                    // stored form
std::printu.stdout["n = " 'n' ", k = " 'k' \n];   // useful form
if ['n' > 10] { … } else.if ['n' > 5] { … } else { … }
loop { … }                                        // until break
loop.while ['total' < 100] { … }
loop.for 'x' in ['xs'] { … }                      // each element of a list
loop.for 'i' in [std::range[1, 'n']] { … }        // counting, inclusive
break;   continue;
return [expr];   return;
std::exit[1];
check { … }   nocheck { … }
add[1, 2];                                        // a call as a statement
```

- **Print** (CyborgPL, decided): `std::print.stdout[pieces]` renders each
  piece in its **stored** form; `std::printu.stdout[pieces]` in its
  **useful** form (§6.7). Pieces are whitespace-separated inside one `[ ]`
  and join into one text with **nothing between them** (**provisional**,
  CyborgPL open: "separator only, or an implicit space?"); `\n` is a
  newline piece; a `"text"` or `'name'` is a piece; **a bare number is an
  operand, never a piece** — `["count: " 10]` is a compile error, write
  `["count: " 'n']`. A piece that is a `list` renders as §6.7 says. No
  newline is added unless a `\n` piece is written. There is no `stderr`
  target in Kespar (**provisional** cut).
- **`if`** (CyborgPL, decided): condition in `[ ]`, must be `bool`, no
  truthiness; `else.if`; braces mandatory.
- **`loop`** (CyborgPL, decided): `loop { }` forever; `loop.while [cond]
  { }`; `loop.for 'x' in [list] { }` binds a fresh `'x'` per element (the
  element's type; for a list of lists, the inner list itself, shared).
  **Counting** (**provisional**, CyborgPL open; Claude recommended a std
  function): `std::range[a, b]` is a value only `loop.for` accepts, both
  ends inclusive, empty when `a > b`; `a` and `b` are evaluated once,
  before the loop; the counter never computes the value after `b`, so a
  range to a type's maximum does not overflow. `'i'` is a `:=` name whose
  initial expression is `a` and which `b` meets (§8.3). `break;` /
  `continue;` apply to the innermost loop; outside one, compile error.
- **`check { }` / `nocheck { }`** (**provisional** — CyborgPL's tier
  syntax is undecided): tier blocks (§8.4), nest, innermost wins, ordinary
  blocks for scoping. Statements outside any are in the default tier.
- **`std::exit[n]`** (CyborgPL, decided): `n` is `uint8`; the process
  exits with it. `MAIN` ending exits 0.
- A call may be a statement; its result, if any, is discarded.

## 6. Expressions and values

### 6.1 `[ ]`, pieces, and lists (CyborgPL, decided)

Inside `[ ]`: **whitespace joins pieces into one value**, **a comma
separates values**. `[10, 20]` is a two-element list; `[1,]` is a
one-element list; `[1]` is the number 1; `["a" 'b' \n]` is one text; `[1 +
2]` is a formula; `[[1, 2], [3, 4]]` is a list of lists. A list literal's
elements must all be one type and there must be at least one (`[]` is a
compile error except as an empty argument list). A `[ ]` inside a `[ ]` is
a value.

### 6.2 Operators and precedence (CyborgPL, decided)

`+ - x xx / mod == !== < > <== >== and or not`, unary `-`. `*`, `^`, `%`,
`!=`, `>=`, `<=`, `++`, `+=` are refused with a message naming the right
spelling. Tightest to loosest:

| | operators | rule |
|---|---|---|
| 1 | `xx` | right-associative: `2 xx 3 xx 2` = 512 |
| 2 | unary `-` | below `xx`: `-2 xx 2` = −4 |
| 3 | `x` `/` `mod` | `x` chains; `/` or `mod` beside `x` `/` `mod` **requires parentheses**; `mod` beside *any* operator requires parentheses |
| 4 | `+` `-` | left |
| 5 | `== !== < > <== >==` | never chained; never beside `not` without parentheses |
| 6 | `not` | |
| 7 | `and` | |
| 8 | `or` | |

Violations are compile errors that say "parenthesise". `( )` always
overrides.

### 6.3 Integer arithmetic

Operands must be the same integer type; the result has that type.

- `+ - x`: the exact result must fit the type, otherwise **overflow** — a
  check site (§8.4). Unary `-` on a signed minimum overflows; on an
  unsigned value it overflows unless the value is 0.
- `xx`: the exponent must be `>= 0` or it is a check site of kind
  **negative exponent**; the exact result must fit or it **overflows**.
  `0 xx 0` = 1.
- `/`: truncates toward zero (**provisional**, CyborgPL open). Divisor
  zero is a check site (**division by zero**). `MIN / -1` overflows.
- `mod`: remainder with the sign of the dividend, so `a == (a / b) x b + (a
  mod b)` (**provisional**, CyborgPL open). Zero divisor: **division by
  zero**. `MIN mod -1` is 0.

### 6.4 Bin arithmetic

`+ - x / xx` on two values of the same bin type, IEEE 754
round-to-nearest-even at that width (a `bin32` operation is performed in
binary32). `xx` is `powf`. `mod` on bins is a compile error. Division by
zero gives `inf`/`NaN`; **bins have no check sites**. Comparisons follow
IEEE (`NaN == NaN` is `false`, `NaN !== NaN` is `true`, ordered
comparisons with `NaN` are `false`).

### 6.5 Comparison, logic, text, lists

- `== !==` on two integers, two bins, two bools, or two strs (by content)
  of the same type. On lists: compile error (**provisional**).
- `< > <== >==` on integers or bins of the same type.
- `and` / `or` on `bool`, **short-circuit**. `not` on `bool`.
- Text is built from pieces (§6.1); there is no `+` on `str`.
- `'xs'[i]`: `i` any integer type, `0 <= i < length` (**provisional**:
  0-based) or a check site (**out of bounds**), on read and on write.
- `std::len[v]`: the length of a `list` or the scalar-value count of a
  `str` (**provisional** spelling). Its result **behaves like a literal**:
  it adopts the integer type its context needs, and with none it is a free
  value (§8.3). Precog knows its set from the list's length bound.
- `std::fill[v, n]`: a list of `n` copies of `v` (**provisional**). `n` is
  any integer type; `n < 0` is an **out of bounds** check site. For a list
  `v`, every slot is the same list.
- Lists are fixed-length; there is no append.

### 6.6 Conversion — `std::to` (**provisional**; CyborgPL open)

`std::to.int32[v]`, `std::to.uint8[v]`, `std::to.bin64[v]`,
`std::to.str.utf8[v]`.

| from → to | rule |
|---|---|
| integer → integer | exact; a value that does not fit **overflows** (check site) |
| integer → bin | nearest representable |
| bin → integer | truncate toward zero; does not fit, or `inf`/`NaN`: **overflows** (check site) |
| bin32 ↔ bin64 | IEEE conversion |
| integer / bin / bool → `str.utf8` | the **useful** rendering (§6.7) |
| str → anything, anything → bool, list ↔ anything | compile error |
| T → T | allowed, no effect |

### 6.7 Rendering — stored vs useful (CyborgPL, decided)

- integers: decimal, `-` for negatives, no padding, no `_`. Same in both
  forms.
- bins, **stored**: the exact decimal expansion of the binary value
  (`0.1` as `bin32` prints `0.100000001490116119384765625`; `1.0` prints
  `1`; integral values print with no `.`), `-0` for negative zero, `inf`,
  `-inf`, `NaN`. **Useful**: Rust's `{}` `Display` at the value's own width
  — the shortest decimal that round-trips, no exponent notation, `.0`
  omitted (**provisional** rendering choice, picked so both Rust engines
  agree trivially).
- bool: `true` / `false`. str: the text itself.
- list: `[` elements rendered in the same form, joined by `, `, `]`;
  nested the same way; an empty list is `[]` (**provisional**).
- **A number piece becomes text in its stored form everywhere**
  (CyborgPL, decided): `var 's' := ["x is " 'x'];` holds the stored
  expansion, and `printu['s']` prints what `'s'` stores. Only `printu`'s
  own direct pieces, and `std::to.str.utf8`, produce the useful form.

## 7. Functions and `MAIN` (CyborgPL, decided)

```
func.int32 add [var.int32 'a', var.int32 'b'] {
    return ['a' + 'b'];
}
func shout [var.str.utf8 'text'] {              // returns nothing
    std::printu.stdout['text' "!" \n];
}
func twice [var 'n'] {                          // untyped parameter and return: Precog picks
    return ['n' x 2];
}
MAIN {
    var 'sum' := [add[10, 20]];
    var 'four' := [twice[2]];
    shout["hi"];
}
```

- `func.T name [params] { }` — the chain narrows `func` by return type.
  `func name` with no type: Precog picks the return type from every
  `return` (§8.3 — the return value is a `:=` name); with no `return`
  anywhere, the function returns nothing and cannot be used as a value.
- Parameters are `var` declarations, comma-separated in one `[ ]`.
  `var 'n'` untyped: the parameter is a `:=` name met by every argument at
  every call site (§8.3). `var.immut.T` is allowed.
- Call: `name[args]`, positional, exact count, exact types (a literal
  adopts the parameter's type). As a value it wears the outer bracket:
  `var 's' := [add[1, 2]];`. As a statement, none.
- With a return type every path must `return [v]`; falling off the end is
  a compile error. `return;` only in a nothing-function.
- Functions are declared at top level only, in any order, may be
  recursive. Calling an undeclared function is a compile error.
- **`MAIN { }`** is the program's block, once per file, unquoted, not a
  function. Ending it exits 0. Top level holds only `func` definitions and
  `MAIN`.
- The `std` Limb's members are exactly: `print.stdout`, `printu.stdout`,
  `read.stdin`, `range`, `len`, `fill`, `to.<type>`, `exit`. Anything else
  after `std::` is a compile error naming it.

## 8. Precog — what the compiler knows and what it checks

Observable rules; `design/precog.md` says how they are met. The reference
interpreter implements §8.4's default-tier behaviour (check everything,
every site, always) and §8.3's forcing; it has no Precog.

### 8.1 Known and bounded

Every value is either **known** — a literal, or computed only from known
values — or **bounded** — a `std::read.stdin` or anything computed from
one. A bounded value's possible set is what its declared type and bounds
allow, propagated exactly through every operation.

### 8.2 The boundary rule

A read must carry an explicit type (§4). Downstream, `:=` is allowed on
bounded values as long as a bound can be proven.

### 8.3 What `:=` picks (the "best" type)

A `:=` name — a `var 'x' :=`, an untyped parameter, an untyped function's
return value, a `loop.for` counter or element, or the free result of
`std::len` — has a **family** fixed by its initial expression (integer,
bin, bool, str, list of a family) and a **width** the compiler picks in two
steps.

**Step 1 — forced by use.** Because nothing converts implicitly, a `:=`
name that ever meets an explicitly typed value takes that value's type. The
meeting places: a binary operator with one explicitly typed operand, an
assignment in either direction, an argument to a typed parameter, an
argument *from* a typed value to an untyped parameter, a `return` in a
typed function, an element beside a typed element in a list literal or
`std::fill`, the other bound in `std::range`. Pieces, comparisons' results
and `std::to` do not force (rendering and conversion are by value). Forcing
is transitive and a name forced to two different types is a compile error
(*"'x' is used as int32 at line 4 and as int64 at line 9"*). A **forced**
name then behaves exactly like an explicitly typed one: every operation on
it is an ordinary check site.

**Step 2 — free names.** A `:=` name forced by nothing is **free**. For a
free integer name Precog computes the exact set of every value the name
ever holds *and* of every expression whose type it decides (`['x' x 3]`,
`['x' + 32700]`, `[-'x']`, …), across the whole program, and picks the
**narrowest** width holding all of them — signed if any value is negative,
else unsigned (**provisional**; CyborgPL says "best for safety and
performance", which in a VM with 64-bit slots reduces to "narrowest that is
safe"). If no 64-bit type holds them, or the set cannot be bounded at all,
it is a **compile error**: *"no integer type holds 'x' (reaches 2^70 at
line 12); write an explicit type"* / *"cannot bound 'x' (grows without
limit in the loop at line 7); write an explicit type"*. Free bin names are
**`bin64`** (**provisional**). `bool` and `str.utf8` have one width. A free
list takes its elements' width by the same rule.

**The consequence, and the point:** on a free name **overflow cannot
happen** — the width was chosen so that it does not — so every operation
typed by a free name has no check, in every tier. The reference
interpreter therefore treats a free integer as an **unbounded integer** and
never overflows on it, a free bin as `bin64`, performs step 1 exactly as
written, and needs no Precog. Width is unobservable except through
overflow, so the two engines agree on every program whose Precog was right,
and any overflow the compiled program reports at a site typed by a free
name is by construction a Precog bug. `kespar check --types` prints every
chosen width.

### 8.4 Check sites and tiers

A **check site** is any place a property can fail: integer `+ - x xx /
mod` and unary `-` (**overflow**), `xx` (**negative exponent**), `/ mod`
(**division by zero**), `'xs'[i]` and `std::fill` (**out of bounds**),
`std::to` into an integer (**overflow**). Each site is in one tier:

| tier | spelled | if Precog proves the property | if it cannot |
|---|---|---|---|
| **default** | nothing | check removed | a run-time check, whatever it costs; compiles |
| **check** | inside `check { }` | check removed | **compile error** with a counterexample input or the missing bound |
| **nocheck** | inside `nocheck { }` | removed | removed, unverified — the programmer's responsibility |

A failing run-time check stops the program with exit code **1** and one
line on stderr, the source line being the operator's:

```
kespar: overflow at line 12
kespar: division by zero at line 12
kespar: out of bounds at line 12
kespar: negative exponent at line 12
```

In a `nocheck` block with its check removed, the behaviour on failure is
**unspecified** (the VM wraps two's-complement and stops on a zero
divisor or bad index anyway; nothing may rely on it). The oracle never
generates a `nocheck` site that can fail. The read contract (§4) and the
step budget are not tier-controlled.

### 8.5 Compile-time execution and the step budget

Precog executes every known value at compile time. A loop or recursion
over known values that never finishes would hang the compiler, so
compile-time execution has a budget **counted in steps** (owner,
2026-09-16: never wall-clock). **Provisional**: 100,000,000 steps per
program, one per evaluated node or executed statement. On exhaustion:
compile error *"compile-time execution exceeded 100000000 steps in the
loop at line 7"* (CyborgPL has not decided this; the POC refuses so the
situation is visible). The run-time VM has **no** budget.

## 9. Exit codes and streams

| exit | meaning | stderr |
|---|---|---|
| 0 | `MAIN` ended, or `std::exit[0]` | — |
| 1 | a check failed (§8.4) | `kespar: <kind> at line N` |
| 2 | read contract violated (§4) | `kespar: bad input for 'name' at line N` |
| 3 | compile error | `error: … at line N` |
| n | `std::exit[n]` | — |

`std::exit[1]`, `[2]`, `[3]` are the programmer's to use; the oracle tells
them apart from Kespar's own by the stderr line. stdout is flushed before
any stop. Nothing else is ever written to stderr.

## 10. A complete program

```
// sum of the first n squares, n from outside
func.int32 square [var.int32 'x'] {
    return ['x' x 'x'];               // proven at every call site: x <== 1000
}

MAIN {
    var.int32 'n' = [std::read.stdin[0, 1000]];
    var 'total' := [0];               // forced to int32 by square[...]
    var 'odd' := [0];                 // free: holds 0..500 → uint16
    loop.for 'i' in [std::range[1, 'n']] {       // 'i' forced to int32 by 'n'
        'total' = ['total' + square['i']];        // proven <== 333_833_500: no check
        if [('i' mod 2) == 1] { 'odd' = ['odd' + 1]; }   // free: no check
    }
    std::printu.stdout["sum = " 'total' ", odd = " 'odd' \n];
}
```

Input `3` prints `sum = 14, odd = 2`. Input `1001` stops with exit 2:
`kespar: bad input for 'n' at line 7`.
