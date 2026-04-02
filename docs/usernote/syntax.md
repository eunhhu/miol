# miol Language Specification

> **miol** — a universal, concise DSL for building fullstack applications.
> WASM-first web runtime, native binary compilation, fine-grained reactivity.

---

## Table of Contents

1. [Philosophy](#philosophy)
2. [Syntax Fundamentals](#syntax-fundamentals)
3. [Type System](#type-system)
4. [Variables & Mutability](#variables--mutability)
5. [Functions](#functions)
6. [Control Flow](#control-flow)
7. [Pattern Matching](#pattern-matching)
8. [Collections](#collections)
9. [Error Handling](#error-handling)
10. [Async / Await](#async--await)
11. [Modules & Imports](#modules--imports)
12. [Node System (`@` / `%`)](#node-system)
13. [Reactivity & Signals](#reactivity--signals)
14. [UI Domain](#ui-domain)
15. [Design Domain](#design-domain)
16. [Server Domain](#server-domain)
17. [Domain Contexts & Validation](#domain-contexts--validation)
18. [Custom Nodes (`define`)](#custom-nodes)
19. [Best Practices](#best-practices)

---

## Philosophy

miol is built on four principles:

- **One syntax, every domain.** UI, server, design tokens, and general logic share a unified grammar. The `@node` / `%property` structure scales from a button to an HTTP server.
- **One abstraction: `define`.** There is no `class`, no `new`, no `this`, no inheritance. `define` is the only way to create reusable abstractions — it replaces components, classes, builders, and modules through composition and closure.
- **Conciseness without magic.** Every abbreviation has a predictable expansion. `$0` is always the first callback parameter. `sig` is always a reactive signal. There are no hidden transforms.
- **Compile-time safety, runtime speed.** Types are inferred like Rust, checked at compile time, and compiled to WASM (web) or native binary. Domain contexts are validated at compile time — you cannot put `@div` inside `@server`.

---

## Syntax Fundamentals

### Node Declaration (`@`)

The `@` prefix declares a structural node. Nodes are the universal building block of miol — they represent UI elements, server routes, design tokens, and custom abstractions alike.

```miol
@identifier param1 param2 ... {
  // children, properties, and executable statements
}
```

Nodes accept **positional tokens** (parsed by keyword, order-independent where applicable), **inline properties** with `%`, and a **body block** `{ }` for children and logic.

### Property Binding (`%`)

The `%` prefix attaches a property to the nearest parent node.

```miol
// Inline (on the same line as the node)
@button "Submit" %onClick={submit()} %disabled={!isValid}

// Inner (inside the node body, applies to the parent node)
@div {
  %style={
    backgroundColor: "red"
  }
  @text "Hello"
}
```

### Three Roles in a Block

Inside any `{ }` block, every line falls into exactly one of three categories:

| Prefix | Role | Example |
|--------|------|---------|
| `@` | Structure — child node | `@text "Hello"` |
| `%` | Configuration — property of the parent | `%onClick={handler()}` |
| *(none)* | Execution — runs when the scope is entered | `let x = 1` |

```miol
@div {
  // @ — structure
  @h1 "Title"

  // % — configuration (applies to parent @div)
  %style={ padding: "1rem" }

  // bare — execution (runs on mount in UI context)
  let sig count: i32 = 0
  @io.out "div mounted"
}
```

### Comments

```miol
// Single-line comment

/* 
  Multi-line
  comment 
*/

/// Documentation comment (attached to the next declaration)
/// Supports markdown formatting.
define Button(label: string) -> @button label rounded-md
```

### Semicolons

Semicolons are **not required**. Line breaks terminate statements. Semicolons are only needed to place multiple statements on one line:

```miol
let a = 1; let b = 2  // two statements, one line
let c = 3              // normal — no semicolon needed
```

---

## Type System

### Primitive Types

| Type | Description |
|------|-------------|
| `i32` | 32-bit signed integer |
| `i64` | 64-bit signed integer |
| `f32` | 32-bit float |
| `f64` | 64-bit float |
| `string` | UTF-8 string |
| `bool` | Boolean |
| `void` | No value / no return value |

When compiled to WASM, numeric types map to their true WASM equivalents (`i32` is a real 32-bit integer). When compiled to native binary, they map to the platform's native types.

### Type Inference

Types are inferred when the right-hand side is unambiguous:

```miol
let x = 42          // inferred as i32
let y = 3.14        // inferred as f64
let name = "miol"   // inferred as string
let flag = true     // inferred as bool
```

Explicit annotation is required when the compiler cannot infer:

```miol
let mut items: Vec<string> = []
```

### Union Types

```miol
type Number = i32 | f64
type Result = string | Error
type Nullable<T> = T?
```

### Nullable Types

Append `?` to make any type nullable:

```miol
let name: string? = void    // nullable string, void means "no value"
let count: i32? = 42        // nullable but has a value
```

`void` serves as both the return type for functions that return nothing and the literal value representing "no value" for nullable types (similar to `null` in other languages).

### Enums

```miol
enum Direction {
  Up
  Down
  Left
  Right
}

enum Status {
  Ok(i32)             // associated value
  Error(string)
}
```

### Structs

Structs are **headless data shapes** — similar to TypeScript interfaces. They describe the shape of a literal object. Structs have no methods, no constructors, no inheritance. They are purely structural types.

miol has no `class`. If you need stateful objects with methods, use [`define` with nested defines](#custom-nodes) instead — it's more explicit, more composable, and avoids the complexity of `this` binding, prototype chains, and inheritance hierarchies.

```miol
struct Point {
  x: i32
  y: i32
}

struct User {
  name: string
  age: i32
  email: string?          // nullable field
  greet: void -> string   // function-typed field
  transform: i32 -> i32   // function-typed field
}

// Instantiation — literal object syntax
let origin = { x: 0, y: 0 }
let user = {
  name: "Kim",
  age: 22,
  email: void,
  greet: _ -> "Hello, I'm {name}",
  transform: x -> x * 2
}
```

### Generics

```miol
type Container<T> = T

struct Pair<A, B> {
  first: A
  second: B
}

struct Node<T> {
  value: T
  next: Node<T>?
}

function identity<T>(x: T): T -> x

let pair: Pair<i32, string> = { first: 1, second: "hello" }
```

### Function Types

Function types use the arrow syntax:

```miol
// Single parameter
type Transform = i32 -> i32

// Multiple parameters
type Add = i32, i32 -> i32

// No parameters
type Factory = void -> string

// Nullable return
type MaybeFind = string -> i32?

// In struct fields
struct Config {
  validate: string -> bool
  onError: string, i32 -> void
}
```

### Tuples

```miol
let pair: (i32, string) = (42, "hello")
let (x, y) = pair     // destructuring

function divmod(a: i32, b: i32): (i32, i32) -> {
  (a / b, a % b)
}
let (quotient, remainder) = divmod(10, 3)
```

---

## Variables & Mutability

miol follows Rust's immutability-by-default philosophy:

```miol
let x = 10          // immutable
let mut y = 20      // mutable
let sig z = 30      // reactive signal (mutable, triggers UI updates)
const PI = 3.14159  // compile-time constant
```

| Keyword | Mutable | Reactive | Scope |
|---------|---------|----------|-------|
| `let` | No | No | Block |
| `let mut` | Yes | No | Block |
| `let sig` | Yes | Yes | Block (tracked by reactivity system) |
| `const` | No | No | Module |

### Destructuring

```miol
// Array destructuring
let [first, second, ...rest] = [1, 2, 3, 4, 5]

// Struct destructuring
let point: Point = { x: 10, y: 20 }
let { x, y } = point

// Tuple destructuring
let (a, b) = (1, 2)

// Nested
let { x, y }: Point = { x: 1, y: 2 }
let [{ name }, ...others] = users
```

---

## Functions

### Declaration

```miol
function add(a: i32, b: i32): i32 -> {
  a + b  // implicit return (last expression)
}

function greet(name: string): string -> {
  return "Hello, {name}"  // explicit return also works
}

// Single-expression shorthand
function double(x: i32): i32 -> x * 2
```

### Callbacks & Closures

```miol
// Named parameter
vec.map(x: i32 -> x * 2)

// Multi-line closure
vec.filter(x: i32 -> {
  let threshold = 10
  x > threshold && x.isEven()
})

// $N shorthand — when any $N token appears, the expression 
// is automatically wrapped as a callback
vec.map($0 * 2)           // same as: x -> x * 2
vec.filter($0 > 10)       // same as: x -> x > 10
items.sort($0.age - $1.age)  // same as: (a, b) -> a.age - b.age
```

**Best Practice:** Use `$0` for simple one-expression transforms. Use named parameters for multi-line closures or when the meaning isn't obvious.

### Pipe Operator

The pipe operator `|>` passes the left-hand value as the first argument to the right-hand function:

```miol
let result = value |> transform |> validate |> format

// Equivalent to:
let result = format(validate(transform(value)))

// Also works with type-attached methods
let nan = x |> f64.isNaN
```

### Async Functions

```miol
async function fetchUser(id: i32): User -> {
  let response = await http.get("/api/users/{id}")
  response.json()
}

// top-level await — no async wrapper needed
let config = await loadConfig()
let db = await Database.connect(config.dbUrl)
```

---

## Control Flow

### If / Else

```miol
if condition {
  doSomething()
} else if otherCondition {
  doOther()
} else {
  fallback()
}
```

### Ternary

```miol
let label = isActive ? "On" : "Off"
```

### For Loops

```miol
// Range iteration (0 to 9)
for i of 0..10 {
  @io.out "{i}"
}

// Inclusive range (0 to 10)
for i of 0..=10 {
  @io.out "{i}"
}

// Collection iteration
for item of items {
  @io.out item.name
}

// With index (enumerated)
for (i, item) of items.enumerate() {
  @io.out "{i}: {item.name}"
}
```

### While

```miol
while condition {
  // ...
}
```

### When (Pattern Matching)

See [Pattern Matching](#pattern-matching) below.

---

## Pattern Matching

`when` is miol's exhaustive pattern matching construct, inspired by Kotlin's `when` with Rust-level expressiveness. `when` can be used both as a statement (for side effects) and as an expression (returning a value).

### Value Matching

```miol
when status {
  200 -> @io.out "OK"
  404 -> @io.out "Not Found"
  500 -> @io.out "Server Error"
  _ -> @io.out "Unknown: {status}"
}
```

### Range Matching

```miol
when score {
  90..=100 -> "A"
  80..90   -> "B"
  70..80   -> "C"
  ..70     -> "F"
  _        -> "Invalid"
}
```

### Or Patterns

```miol
when x {
  1 | 2 | 3 -> @io.out "small"
  _ -> @io.out "other"
}
```

### Struct Destructuring

```miol
let point = Point { x: 1, y: 2 }

when point {
  Point { x: 0, y }    -> @io.out "on y-axis at {y}"
  Point { x, y: 0 }    -> @io.out "on x-axis at {x}"
  Point { x, y } if x == y -> @io.out "on diagonal"
  _                     -> @io.out "somewhere else"
}
```

### Enum Destructuring

```miol
when result {
  Status.Ok(code)    -> @io.out "Success: {code}"
  Status.Error(msg)  -> @io.out "Failed: {msg}"
}
```

**Best Practice:** Always handle `_` (wildcard) to ensure exhaustiveness. The compiler will warn if patterns are not exhaustive for enums.

---

## Collections

### Vec (Dynamic Array)

```miol
let mut numbers: Vec<i32> = []
numbers.push(1)
numbers.push(2)
numbers.pop()         // returns i32?
numbers.len()         // i32
numbers.clear()

// Functional operations
let doubled = numbers.map($0 * 2)
let evens = numbers.filter($0 % 2 == 0)
let sum = numbers.reduce(0, $0 + $1)

// Literal initialization
let primes = [2, 3, 5, 7, 11]
```

### HashMap

```miol
let mut scores: HashMap<string, i32> = #{}
scores.insert("alice", 100)
scores.insert("bob", 85)
scores.get("alice")       // i32?
scores.remove("bob")
scores.clear()
scores.len()
scores.keys()             // Vec<string>
scores.values()           // Vec<i32>

// Literal initialization
let config = #{
  "host": "localhost"
  "port": "8080"
}
```

### Iteration

```miol
for (key, value) of scores {
  @io.out "{key}: {value}"
}

for item of vec {
  @io.out item
}
```

---

## Error Handling

miol uses `try` / `catch` blocks for error handling:

```miol
try {
  let data = await fetchData()
  process(data)
} catch e {
  @io.out "Error: {e.message}"
}
```

### Typed Catch

```miol
try {
  let user = await db.findUser(id)
} catch e: NotFoundError {
  @io.out "User not found"
} catch e: DatabaseError {
  @io.out "DB error: {e.message}"
} catch e {
  @io.out "Unknown error: {e.message}"
}
```

### Try in Expressions

```miol
let user = try db.findUser(id) catch {
  User { name: "anonymous", age: 0 }
}
```

**Best Practice:** Prefer specific catch clauses over generic ones. In server routes, always catch errors and return appropriate HTTP status codes.

---

## Async / Await

### Basics

Functions that perform I/O or network operations are declared `async`:

```miol
async function fetchUser(id: i32): User -> {
  let res = await http.get("/api/users/{id}")
  res.json()
}
```

### Top-Level Await

`await` works **everywhere** — no `async` wrapper required at the top level:

```miol
let config = await loadConfig()
let db = await Database.connect(config.dbUrl)

@server {
  @listen config.port
}
```

### Concurrent Execution

```miol
// Parallel fetch
let (users, posts) = await (fetchUsers(), fetchPosts())

// Or explicitly
let usersFuture = fetchUsers()
let postsFuture = fetchPosts()
let users = await usersFuture
let posts = await postsFuture
```

**Best Practice:** Use concurrent tuple await for independent async operations that can run in parallel.

---

## Modules & Imports

### Import Syntax

miol uses dot-path imports inspired by Python and Rust:

```miol
// Single import
import libs.counter.myFunc

// Multiple imports from the same module
import components.{Button, Input, Card}

// Aliased import
import libs.http.Client as HttpClient

// Wildcard (use sparingly)
import utils.*

// Standard library
import @std.io
import @std.collections.{Vec, HashMap}

// External packages
import @pkg.jwt
import @pkg.database.postgres
```

### Module Structure

Each `.miol` file is a module. The file path maps directly to the import path:

```
project/
├── main.miol              // entry point
├── components/
│   ├── Button.miol        // import components.Button
│   ├── Input.miol         // import components.Input
│   └── Card.miol          // import components.Card
├── libs/
│   ├── counter.miol       // import libs.counter
│   └── http.miol          // import libs.http
└── pages/
    └── Home.miol          // import pages.Home
```

### Exports

Top-level declarations are private by default. Use `pub` to export:

```miol
pub struct User {
  name: string
  age: i32
}

pub function greet(name: string): string -> "Hello, {name}"

pub define Button(label: string) -> @button label rounded-md

// Private — only accessible within this module
function internalHelper(): void -> { ... }
```

---

## Node System

The `@` / `%` system is the core abstraction of miol. Every domain (UI, server, design) is expressed through the same node grammar.

### `@` — Structural Nodes

```miol
@identifier tokens... {
  // children and logic
}
```

Nodes can carry:
- **Positional tokens**: parsed by keyword (order-independent where applicable)
- **String literals**: `"text content"`
- **Tailwind classes**: `rounded-md flex items-center` (in UI context)
- **Inline `%` properties**: `%key=value`

### `%` — Properties

Properties configure the node they belong to:

```miol
// Inline — on the same line (single expression)
@button "Click" %onClick=handler() %disabled=false

// Inner — inside the block, applies to parent
@div {
  %class="container"
  %style={
    display: "flex",
    gap: "1rem"
  }
  @text "Content"
}

// Multi-line block — use { } when the value spans multiple statements
%onClick={
  counter += 1
  @io.out "clicked"
}
```

### `@io` — Standard I/O

```miol
@io.out "Hello, world"        // stdout
@io.err "Something went wrong" // stderr
```

### `@env` — Environment Variables

`@env` reads an environment variable and returns a `string`. Use type conversion if a different type is needed.

```miol
let port = @env PORT           // string
let secret = @env JWT_SECRET   // string

// Inline usage
@listen (@env PORT)
```

---

## Reactivity & Signals

### Signal Declaration

```miol
let sig count: i32 = 0
```

A `sig` variable is **mutable** and **reactive**. When its value changes, any UI node or derived signal that depends on it is automatically updated.

### Reading & Writing

Signals are read and written like normal variables — no special accessor needed:

```miol
let sig count: i32 = 0

// Read
@text "Count: {count}"

// Write
count += 1
count = 0
```

### Derived Signals

Any `sig` whose initial value references another `sig` is automatically derived:

```miol
let sig count: i32 = 0
let sig doubled: i32 = count * 2      // auto-derived
let sig label: string = "Count: {count}"  // auto-derived

// doubled and label update whenever count changes
```

### Fine-Grained Updates

miol's reactivity is fine-grained: when `count` changes, only the specific DOM nodes that reference `count` are updated — not the entire component tree.

```miol
define Counter() -> @div {
  let sig count: i32 = 0

  // Only this text node re-renders when count changes
  @text "Count: {count}"

  // This text node never re-renders
  @text "This is static"

  @button "+" %onClick={count += 1}
}
```

### Signals in Collections

```miol
let sig items: Vec<string> = ["a", "b", "c"]

// Mutating the collection triggers updates
items.push("d")
items.pop()

// Derived from collection
let sig itemCount = items.len()
```

---

## UI Domain

The UI domain is active inside `@html`, `@body`, and UI-specific nodes like `@div`, `@vstack`, `@hstack`.

### HTML Structure

```miol
let page: html = @html {
  @head {
    @title "My Application"
    @meta description "A miol app"
    @meta viewport "width=device-width, initial-scale=1"
  }

  @body {
    @div flex flex-col min-h-screen {
      @Header
      @main flex-1 {
        @Outlet
      }
      @Footer
    }
  }
}
```

### Elements & Tailwind

HTML elements are nodes. Tailwind classes are positional tokens — no `class=` needed:

```miol
@div flex flex-col gap-4 p-6 {
  @h1 text-2xl font-bold "Welcome"
  @p text-gray-500 "This is a paragraph"
  @button bg-blue-500 text-white px-4 py-2 rounded-md "Click me"
}
```

### Layout Shorthands

```miol
@vstack gap-4 {       // vertical stack (flex flex-col)
  @text "Top"
  @text "Bottom"
}

@hstack gap-2 {       // horizontal stack (flex flex-row)
  @text "Left"
  @text "Right"
}
```

### Event Handling

```miol
// Inline
@button "Click" %onClick={count += 1}

// Block
@button "Submit" {
  %onClick={
    let result = await submitForm()
    if result.ok {
      navigate("/success")
    }
  }
}
```

### Conditional Rendering

```miol
@div {
  if isLoggedIn {
    @text "Welcome, {username}"
    @button "Logout" %onClick={logout()}
  } else {
    @button "Login" %onClick={showLogin()}
  }
}
```

### List Rendering

```miol
@ul {
  for item of items {
    @li "{item.name} — {item.description}"
  }
}

// With index
@ol {
  for (i, task) of tasks.enumerate() {
    @li "#{i + 1}: {task.title}"
  }
}
```

### Children

Components receive children via `@children`:

```miol
define Card(title: string) -> @div rounded-lg shadow-md p-4 {
  @h2 font-bold text-lg "{title}"
  @div mt-2 {
    @children
  }
}

// Usage
@Card %title="Profile" {
  @text "Card content goes here"
  @button "Action"
}
```

### Lifecycle

Lifecycle hooks are `%` properties:

```miol
define Timer() -> @div {
  let sig seconds: i32 = 0
  let mut interval: Interval? = void

  %onMount={
    interval = @io.interval 1000 {
      seconds += 1
    }
  }

  %onUnmount={
    interval?.clear()
  }

  @text "{seconds}s elapsed"
}
```

| Hook | Trigger |
|------|---------|
| `%onMount` | Node is added to the DOM |
| `%onUnmount` | Node is removed from the DOM |

### Inline Styles

```miol
@div {
  %style={
    backgroundColor: "red",     // camelCase
    // "background-color": "red", // kebab-case also works
    padding: "1rem"
  }
  @text "Styled div"
}
```

**Best Practice:** Prefer Tailwind classes for styling. Use `%style` only for dynamic values that depend on signals or computed state.

### String Interpolation in Templates

```miol
let sig name: string = "World"

@text "Hello, {name}"          // reactive — updates when name changes
@h1 "Page {currentPage} of {totalPages}"
```

---

## Design Domain

The `@design` block defines design tokens — colors, sizes, fonts, and themes.

```miol
@design {
  // Theme-specific tokens
  @theme light {
    @color primary #1a1a1a
    @color foreground #ffffff
    @color background #f5f5f5
  }

  @theme dark {
    @color primary #ffffff
    @color foreground #1a1a1a
    @color background #0a0a0a
  }

  // Global tokens (theme-independent)
  @color accent #3b82f6
  @color error #ef4444

  @size base 16px
  @size sm 14px
  @size lg 20px
  @size radius 8px

  @font sans "Inter, system-ui, sans-serif" 16px weight-400 line-1.5
  @font mono "JetBrains Mono, monospace" 14px weight-400 line-1.6
}
```

### Using Design Tokens

Design tokens are referenced as Tailwind-style classes in UI nodes:

```miol
@h1 text-primary bg-background font-sans "Hello"
@p text-foreground text-base "Body text"
@span text-error text-sm "Error message"
```

**Best Practice:** Define all colors as tokens in `@design`. Never use hardcoded hex values in UI nodes.

---

## Server Domain

The `@server` block defines an HTTP server with routes, middleware, and request handling.

### Basic Server

```miol
@server {
  @listen 8080

  @route GET / {
    @serve ./public
  }
}
```

### Routes

```miol
@server {
  @listen 8080

  // Token order is flexible — method and path are parsed by keyword
  @route GET /api/users {
    return @response 200 {
      "users": []
    }
  }

  @route POST /api/users {
    let { name, email } = @body
    let user = await db.createUser(name, email)
    return @response 201 { "user": user }
  }

  // Wildcard
  @route * {
    @serve htmlString
  }
}
```

### Nested Routes

Routes nest naturally. Child routes inherit the parent's path prefix and middleware:

```miol
@server {
  @listen 8080

  @route /api {

    @before {
      @io.out "API request: {@method} {@path}"
    }

    @route GET /users {
      // handles GET /api/users
      let skip = @query "skip"
      let limit = @query "limit"
      let users = await db.findUsers(skip, limit)
      return @response 200 { "users": users }
    }

    @route GET /users/:id {
      // handles GET /api/users/:id
      let id = @param "id"
      let user = await db.findUser(id)
      return @response 200 { "user": user }
    }

    @route POST /users {
      // handles POST /api/users
      let { name, email } = @body
      let user = await db.createUser(name, email)
      return @response 201 { "user": user }
    }
  }
}
```

### Request Accessors

| Accessor | Returns | Description |
|----------|---------|-------------|
| `@body` | parsed body | Request body (JSON parsed) |
| `@param "key"` | `string?` | URL path parameter (`:id` in `/users/:id`) |
| `@query "key"` | `string?` | Query string parameter (`?skip=10`) |
| `@header "key"` | `string?` | Request header value |
| `@method` | `string` | HTTP method |
| `@path` | `string` | Request path |
| `@context "key"` | any | Value set by `@before` middleware |

```miol
// @param — path parameters from the route pattern
@route GET /users/:id {
  let id = @param "id"        // from /users/42 → "42"
}

// @query — query string parameters
@route GET /users {
  let skip = @query "skip"    // from /users?skip=10 → "10"
  let limit = @query "limit"  // from /users?limit=20 → "20"
}

// @body — parsed request body
@route POST /users {
  let { name, email } = @body // JSON body parsed
}

// @header — request headers
@route * {
  let token = @header "Authorization"
  let contentType = @header "Content-Type"
}
```

### Response

Responses are returned with `return @response`:

```miol
// Simple
return @response 200 { "message": "OK" }

// With headers
return @response 200 %header={
  "Content-Type": "application/json"
  "X-Custom": "value"
} {
  "data": result
}

// Early return (guard clause)
if !authorized {
  return @response 401 { "error": "Unauthorized" }
}

// Empty body
return @response 204 {}
```

`@response` is always used with `return` — it terminates the route handler and sends the HTTP response.

### Middleware

```miol
@route /api {

  // Runs before every child route
  @before {
    let token = @header "Authorization"
    let verified = await jwt.verify(token, SECRET)
    if !verified {
      return @response 401 { "error": "Unauthorized" }
    }
    // Pass data to route handlers via @context
    return @context {
      userId: verified.sub
    }
  }

  // Runs after every child route
  @after {
    @io.out "Request completed"
  }

  @route GET /profile {
    let userId = @context "userId"
    let user = await db.findUser(userId)
    return @response 200 { "user": user }
  }
}
```

### Serving Static Files & HTML

```miol
@route GET / {
  @serve ./public             // static directory
}

@route GET /app {
  @serve htmlString           // miol html node
}

@route GET /js {
  @serve ./public/bundle.js   // specific file
}
```

### Routes as Variables — Fullstack RPC

Routes assigned to variables become **callable endpoints** from the UI domain. This is miol's built-in fullstack RPC — no separate API client, no manual fetch URLs, no code generation step.

```miol
@server {
  @route / {
    // Assign a route to a variable
    let userService = @route GET /api/user {
      let users = await db.findAll()
      return @response 200 { "users": users }
    }

    let createUser = @route POST /api/user {
      let { name, email } = @body
      let user = await db.create(name, email)
      return @response 201 { "user": user }
    }
  }

  @listen 8000
}

@html {
  @body {
    @div {
      let sig data = void

      // Call the server route directly from UI — no manual fetch() needed
      data = await userService.fetch()

      // The compiler knows the route's URL, method, and response shape
      // userService.fetch() compiles to: fetch("/api/user", { method: "GET" })

      if data != void {
        for user of data.users {
          @text "{user.name}"
        }
      } else {
        @text "Loading..."
      }
    }

    @button "Add User" %onClick={
      await createUser.fetch(%body={
        name: "Kim",
        email: "kim@example.com"
      })
      // Refresh data
      data = await userService.fetch()
    }
  }
}
```

**How it works:**

| Concept | Description |
|---------|-------------|
| `let x = @route ...` | Assigns a route to a variable, making it a callable reference |
| `x.fetch()` | Calls the route from the client — compiles to a `fetch()` with the correct URL and method |
| `x.fetch(%body={...})` | Sends a request body (for POST/PUT/PATCH) |
| `x.fetch(%query={...})` | Appends query parameters |
| `x.fetch(%header={...})` | Adds custom headers |

**Why this matters:**

- **Type safety across the boundary.** The compiler knows the response shape from `@response`, so `data.users` is type-checked at compile time.
- **No URL strings in UI code.** Route paths are an implementation detail — the UI references the variable, not the URL.
- **Refactoring safety.** Rename the route path, and all `.fetch()` calls still work because they reference the variable, not a hardcoded string.
- **Zero boilerplate.** No API client library, no OpenAPI spec, no codegen step. The connection between server and client is the variable binding.

### Nested Route References

```miol
@server {
  @route /api {
    let getUsers = @route GET /users {
      return @response 200 { "users": await db.findAll() }
    }

    let getUser = @route GET /users/:id {
      let id = @param "id"
      return @response 200 { "user": await db.findUser(id) }
    }

    let deleteUser = @route DELETE /users/:id {
      let id = @param "id"
      await db.deleteUser(id)
      return @response 204 {}
    }
  }

  @listen 8000
}

// In UI
@html {
  @body {
    @div {
      // Fetch all users
      let sig users = await getUsers.fetch()

      // Fetch a specific user — path params passed via %param
      let sig profile = await getUser.fetch(%param={ id: "42" })

      // Delete with confirmation
      @button "Delete" %onClick={
        await deleteUser.fetch(%param={ id: profile.user.id })
        users = await getUsers.fetch()  // refresh
      }
    }
  }
}
```

### Server as Function

Servers can be created dynamically:

```miol
function myServer(port: i32, root: string) -> @server {
  @listen port
  @route * {
    @serve root
  }
}

myServer(8080, "./public")
myServer(3000, "./admin")
```

---

## Domain Contexts & Validation

miol enforces **compile-time domain validation**. Each top-level block (`@html`, `@server`, `@design`) defines a context that restricts which `@` nodes are valid inside it.

```miol
// ✅ Valid — each node belongs to its correct domain
@server {
  @listen 8080
  @route / { @serve page }
}

@html {
  @body {
    @div { @text "Hello" }
  }
}

@design {
  @theme dark {
    @color primary #fff
  }
}
```

```miol
// ❌ Compile errors — domain mismatch
@server {
  @div { ... }           // ERROR: @div is not valid in server context
}

@html {
  @body {
    @listen 8080         // ERROR: @listen is not valid in UI context
    @route / { ... }     // ERROR: @route is not valid in UI context
  }
}

@design {
  @route / { ... }       // ERROR: @route is not valid in design context
}
```

### Cross-Domain References

Use variables to bridge domains:

```miol
let page = @html {
  @body {
    @div { @text "Hello" }
  }
}

@server {
  @listen 8080
  @route / {
    @serve page   // reference, not inline — keeps domains separate
  }
}
```

---

## Custom Nodes (`define`)

### Why `define`, Not `class`

miol has no `class` keyword. There is no `new`, no `this`, no inheritance, no prototypes. This is intentional.

`define` replaces every role that `class` traditionally fills:

| Traditional OOP | miol equivalent |
|----------------|-----------------|
| Class with methods | `define` with nested `define`s |
| Constructor | `define` parameters |
| Instance state | `let` / `let mut` / `let sig` inside `define` |
| Encapsulation | Closure scope (inner variables are private by default) |
| Polymorphism | `define` returning different `@` nodes based on params |
| Composition | `@children` + nested `define` |
| Singleton | Top-level `define` called once |

The reasoning: miol is a **node-oriented language**. Everything is either a node (`@`), a property (`%`), or a statement. Classes introduce a parallel object system that competes with the node tree. `define` keeps everything in one unified model.

### Basic Syntax

```miol
define Name(params...) -> returnNode {
  // body
}
```

- **`Name`**: PascalCase by convention for UI components, camelCase for utilities
- **`params`**: typed parameters, received as `%` properties at call site
- **`-> returnNode`**: the root node or value this define produces
- **body**: children, properties, logic — same three-role rules as any `{ }` block

### Simple Component

```miol
define Button(label: string, variant: string?) -> @button label rounded-md {
  when variant {
    "primary"   -> %class="bg-blue-500 text-white"
    "danger"    -> %class="bg-red-500 text-white"
    _           -> %class="bg-gray-200 text-gray-800"
  }
}

// Usage — invoked as a node with @
@Button %label="Submit" %variant="primary"
@Button %label="Cancel"
```

### Positional Tokens with `@token`

`define` can inspect positional tokens (bare words) from the invocation line. `@token` checks if a specific token is present:

```miol
define Alert(message: string) -> @div p-4 rounded-md {
  if @token warning {
    %class="bg-yellow-100 text-yellow-800"
  } else if @token error {
    %class="bg-red-100 text-red-800"
  } else {
    %class="bg-blue-100 text-blue-800"
  }
  @text message
}

// Usage — tokens are bare words after @Identifier
@Alert warning %message="Check your input"
@Alert error %message="Something failed"
@Alert %message="Just so you know"
```

`@token` with a regex pattern matches dynamic tokens:

```miol
define Listen() -> {
  port = @token \d+    // captures the first numeric token
}

// Usage
@Listen 8080           // port = 8080
```

### Children with `@children`

Any nodes placed inside the invocation block are available as `@children` inside the define:

```miol
define Card(title: string) -> @div rounded-lg shadow-md p-4 {
  @h2 font-bold text-lg "{title}"
  @div mt-2 {
    @children
  }
}

// Usage — block contents become @children
@Card %title="Settings" {
  @text "Card body content"
  @button "Save"
}

// No children — @children renders nothing
@Card %title="Empty Card"
```

### Inner State

Variables declared inside `define` are **private to that instance**. Each invocation gets its own closure:

```miol
define Counter(initial: i32?) -> @div {
  let sig count: i32 = initial ?? 0

  @text "Count: {count}"
  @hstack gap-2 {
    @button "-" %onClick={count -= 1}
    @button "+" %onClick={count += 1}
    @button "Reset" %onClick={count = initial ?? 0}
  }
}

// Each instance has independent state
@Counter %initial={0}     // its own count
@Counter %initial={100}   // its own count, starts at 100
```

### Nested `define` — The `class` Killer

`define` blocks can contain nested `define`s, creating inner APIs. This is how miol replaces classes with methods:

```miol
define createServer() -> {
  let sig port: i32 = 8000
  let mut routes: Vec<Route> = []
  let server_instance = @io.serve(port)

  define listen(p: i32) -> {
    port = p
  }

  define route(method: string, path: string, handler: _ -> void) -> {
    routes.push(Route { method, path, handler })
  }

  define start() -> {
    @io.out "Server listening on port {port}"
    for r of routes {
      server_instance.register(r)
    }
  }

  // Return an interface — callers see listen, route, start
  // but not port, routes, or server_instance
  return { listen, route, start }
}

// Usage — looks like a class instance, but it's just closures
let app = createServer()
app.listen(3000)
app.route("GET", "/", _ -> return @response 200 { "ok": true })
app.start()
```

This pattern gives you:
- **Encapsulation**: `port`, `routes`, `server_instance` are not accessible outside
- **State**: each `createServer()` call gets its own isolated state
- **Methods**: `listen`, `route`, `start` are just functions that close over the shared state
- **No `this`**: no binding confusion, no `this` in callbacks

### `define` as Builder Pattern

```miol
define createQuery(table: string) -> {
  let mut conditions: Vec<string> = []
  let mut limit_val: i32? = void
  let mut order_by: string? = void

  define where(condition: string) -> {
    conditions.push(condition)
  }

  define limit(n: i32) -> {
    limit_val = n
  }

  define orderBy(field: string) -> {
    order_by = field
  }

  define build(): string -> {
    let mut sql = "SELECT * FROM {table}"
    if conditions.len() > 0 {
      sql = sql + " WHERE " + conditions.join(" AND ")
    }
    if order_by != void {
      sql = sql + " ORDER BY {order_by}"
    }
    if limit_val != void {
      sql = sql + " LIMIT {limit_val}"
    }
    sql
  }

  return { where, limit, orderBy, build }
}

let q = createQuery("users")
q.where("age > 18")
q.where("active = true")
q.orderBy("name")
q.limit(10)
let sql = q.build()
// → "SELECT * FROM users WHERE age > 18 AND active = true ORDER BY name LIMIT 10"
```

### `define` as Domain Primitives

The built-in `@server`, `@route`, etc. are conceptually `define` blocks with domain context. You can create your own domain primitives:

```miol
define ApiGroup(prefix: string) -> {

  define get(path: string, handler: _ -> void) -> {
    @route GET {prefix}{path} {
      try {
        handler()
      } catch e {
        return @response 500 { "error": e.message }
      }
    }
  }

  define post(path: string, handler: _ -> void) -> {
    @route POST {prefix}{path} {
      try {
        handler()
      } catch e {
        return @response 500 { "error": e.message }
      }
    }
  }

  return { get, post }
}

// Usage
@server {
  @listen 8080

  let users = ApiGroup("/api/users")

  users.get("/", _ -> {
    let all = await db.findAllUsers()
    return @response 200 { "users": all }
  })

  users.post("/", _ -> {
    let { name, email } = @body
    let user = await db.createUser(name, email)
    return @response 201 { "user": user }
  })
}
```

### `define` as State Machine

```miol
define createFetcher<T>(fetchFn: _ -> T) -> {
  let sig state: string = "idle"
  let sig data: T? = void
  let sig error: string? = void

  define execute() -> {
    state = "loading"
    data = void
    error = void
    try {
      data = await fetchFn()
      state = "success"
    } catch e {
      error = e.message
      state = "error"
    }
  }

  define reset() -> {
    state = "idle"
    data = void
    error = void
  }

  return { state, data, error, execute, reset }
}

// Usage in UI
define UserProfile(userId: i32) -> @div {
  let fetcher = createFetcher(_ -> http.get("/api/users/{userId}"))

  %onMount={
    fetcher.execute()
  }

  when fetcher.state {
    "idle"    -> @text "Ready"
    "loading" -> @text "Loading..."
    "success" -> {
      @h1 "{fetcher.data.name}"
      @p "{fetcher.data.email}"
    }
    "error"   -> @text text-red-500 "Error: {fetcher.error}"
    _         -> @text "Unknown state"
  }
}
```

### `define` with Generic Types

```miol
define List<T>(items: Vec<T>, renderItem: T -> void) -> @ul {
  for item of items {
    @li {
      renderItem(item)
    }
  }
}

// Usage
@List<User> %items={users} %renderItem={user: User -> {
  @text "{user.name} ({user.email})"
}}

```

### Exported Definitions

```miol
// components/Button.miol
pub define PrimaryButton(label: string) -> @button label {
  %class="bg-blue-500 text-white px-4 py-2 rounded-md hover:bg-blue-600"
}

pub define DangerButton(label: string) -> @button label {
  %class="bg-red-500 text-white px-4 py-2 rounded-md hover:bg-red-600"
}

// Private — not accessible outside this file
define baseButtonStyles() -> "px-4 py-2 rounded-md font-medium"
```

### Summary: `define` Capabilities

| Capability | Pattern |
|-----------|---------|
| UI Component | `define Name() -> @div { ... }` |
| Utility Function | `define helper() -> { return value }` |
| Stateful Object | `define create() -> { let state; return { methods } }` |
| Builder | `define builder() -> { return { chain, build } }` |
| State Machine | `define machine() -> { let sig state; return { state, transitions } }` |
| Domain Primitive | `define group() -> { define innerRoute(); return api }` |
| Higher-Order | `define hoc<T>(component: T) -> @div { ... }` |

---

## Best Practices

### 1. File Organization

```
project/
├── main.miol                // entry: server + wiring
├── design.miol              // @design tokens
├── components/
│   ├── Button.miol          // pub define Button
│   ├── Card.miol
│   ├── Input.miol
│   └── Layout.miol
├── pages/
│   ├── Home.miol
│   ├── About.miol
│   └── Dashboard.miol
├── server/
│   ├── routes.miol          // route definitions
│   ├── middleware.miol       // @before / @after blocks
│   └── db.miol              // database helpers
└── libs/
    ├── auth.miol
    └── validation.miol
```

### 2. Signal Hygiene

```miol
// ✅ Good — signals only for values that drive UI updates
let sig count: i32 = 0
let sig username: string = ""

// ❌ Bad — using sig for non-reactive data
let sig API_URL: string = "https://api.example.com"  // use const instead
let sig tempCalc: i32 = someExpensiveCalc()           // use let instead
```

### 3. Keep `define` Blocks Focused

```miol
// ✅ Good — one responsibility per define
define UserAvatar(url: string, size: i32) -> @img %src={url} rounded-full {
  %style={
    width: "{size}px"
    height: "{size}px"
  }
}

define UserCard(user: User) -> @div flex items-center gap-3 {
  @UserAvatar %url={user.avatarUrl} %size={48}
  @div {
    @text font-bold "{user.name}"
    @text text-gray-500 text-sm "{user.email}"
  }
}

// ❌ Bad — doing too much in one define
define UserSection(users: Vec<User>) -> @div {
  // fetching, filtering, rendering, pagination... all in one block
}
```

### 4. Error Handling in Server Routes

```miol
// ✅ Good — always handle errors in routes
@route POST /api/users {
  try {
    let { name, email } = @body
    let user = await db.createUser(name, email)
    return @response 201 { "user": user }
  } catch e: ValidationError {
    return @response 400 { "error": e.message }
  } catch e {
    @io.err "Unexpected: {e.message}"
    return @response 500 { "error": "Internal server error" }
  }
}

// ❌ Bad — unhandled errors crash the server
@route POST /api/users {
  let { name, email } = @body       // throws if body is malformed
  let user = await db.createUser(name, email)  // throws on DB error
  return @response 201 { "user": user }
}
```

### 5. Use Design Tokens, Not Hardcoded Values

```miol
// ✅ Good
@design {
  @color primary #3b82f6
  @color text-muted #6b7280
  @size radius-md 8px
}

@button bg-primary text-white "Submit"
@p text-text-muted "Helper text"

// ❌ Bad
@button %style={ backgroundColor: "#3b82f6", color: "#ffffff" } "Submit"
@p %style={ color: "#6b7280" } "Helper text"
```

### 6. Prefer Composition Over Complexity

```miol
// ✅ Good — compose small defines
define IconButton(icon: string, label: string) -> @button flex items-center gap-2 {
  @Icon %name={icon}
  @text "{label}"
}

define DangerButton(label: string) -> @button bg-red-500 text-white rounded-md {
  @text "{label}"
}

// ✅ Good — use define for repeated patterns
define ApiRoute(method: string, path: string) -> @route {
  @before {
    let token = @header "Authorization"
    if !token {
      return @response 401 { "error": "Unauthorized" }
    }
  }
  @children
}
```

### 7. Async Best Practices

```miol
// ✅ Good — parallel fetching
let (users, posts) = await (fetchUsers(), fetchPosts())

// ❌ Bad — sequential when parallel is possible
let users = await fetchUsers()
let posts = await fetchPosts()  // waits for users to finish first

// ✅ Good — error handling on async
let user = try await fetchUser(id) catch {
  @io.err "Failed to fetch user {id}"
  User { name: "unknown", age: 0 }
}
```

### 8. Domain Separation

```miol
// ✅ Good — each domain in its own file or clearly separated
// design.miol
@design {
  @theme light { ... }
  @theme dark { ... }
}

// pages/Home.miol
pub define HomePage() -> @html {
  @body { ... }
}

// main.miol
import design.*
import pages.Home.HomePage

@server {
  @listen 8080
  @route / { @serve HomePage() }
}

// ❌ Bad — everything in one massive file with domains interleaved
```

---

## Full Example: Todo Application

```miol
// design.miol
@design {
  @theme light {
    @color primary #1a1a1a
    @color surface #ffffff
    @color border #e5e7eb
    @color text-muted #6b7280
  }

  @theme dark {
    @color primary #f5f5f5
    @color surface #1f2937
    @color border #374151
    @color text-muted #9ca3af
  }

  @font sans "Inter, sans-serif" 16px weight-400 line-1.5
}

// components/TodoItem.miol
import @std.io

pub define TodoItem(todo: Todo) -> @li flex items-center gap-3 p-3 border-b border-border {
  @input %type="checkbox" %checked={todo.done} %onChange={
    todo.done = !todo.done
  }

  if todo.done {
    @span text-text-muted line-through "{todo.title}"
  } else {
    @span text-primary "{todo.title}"
  }

  @button text-red-500 hover:text-red-700 "×" %onClick={
    todo.deleted = true
  }
}

// pages/Home.miol
import components.TodoItem

struct Todo {
  title: string
  done: bool
  deleted: bool
}

pub define HomePage() -> @html {
  @head {
    @title "miol Todo"
    @meta viewport "width=device-width, initial-scale=1"
  }

  @body font-sans bg-surface text-primary {
    @div max-w-md mx-auto py-8 {
      @h1 text-2xl font-bold mb-4 "miol Todo"

      let sig todos: Vec<Todo> = []
      let sig input: string = ""

      // Derived
      let sig remaining: i32 = todos.filter($0.done == false).len()

      @div flex gap-2 mb-4 {
        @input flex-1 border border-border rounded-md px-3 py-2
          %type="text"
          %placeholder="What needs to be done?"
          %value={input}
          %onInput={input = $0.target.value}
          %onKeyDown={
            if $0.key == "Enter" && input.len() > 0 {
              todos.push(Todo { title: input, done: false, deleted: false })
              input = ""
            }
          }

        @button bg-primary text-surface px-4 py-2 rounded-md "Add" %onClick={
          if input.len() > 0 {
            todos.push(Todo { title: input, done: false, deleted: false })
            input = ""
          }
        }
      }

      @ul {
        for todo of todos {
          if !todo.deleted {
            @TodoItem %todo={todo}
          }
        }
      }

      @p text-text-muted text-sm mt-4 "{remaining} items remaining"
    }
  }
}

// main.miol
import pages.Home.HomePage

let PORT = @env PORT

@server {
  @listen PORT

  @route GET / {
    @serve HomePage()
  }

  @route GET /static {
    @serve ./public
  }
}
```