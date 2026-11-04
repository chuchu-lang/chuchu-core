# Modules and imports

One `.chuchu` file is one module. Compile an entry file with the existing command:

```sh
cheeku examples/modules/main.chuchu -o program
```

Library files start with a module declaration:

```chuchu
module math

export function add(a: int, b: int) be int {
    return a + b
}

export record Point {
    x: int
    y: int
}
```

Import a file with an explicit local alias:

```chuchu
import "./math.chuchu" as math

function origin() be math::Point {
    return math::Point {
        x: 0
        y: 0
    }
}
show math::add(2, 3)
```

## Rules

- A module declaration, when present, is the first statement. It is required in
  imported files and optional in the entry file. Its name is a descriptive label;
  canonical file paths determine identity, and the import alias determines access.
- Imports appear after the module declaration and before all other declarations
  or statements. Paths are nonempty relative `.chuchu` paths, resolved against the
  importing file's directory, not the process working directory. There is no
  implicit extension, package search, or network lookup.
- Only functions and records can be exported. Declarations are private by default.
  Exported signatures and record fields cannot expose private record types.
- Qualified calls, record initializers, and record type annotations use `alias::name`.
  Imports expose only the target module's own explicitly exported declarations.
  Imports are not re-exported. There are no wildcard or selective imports.
- Imported files contain only imports, functions, and records. Only the entry
  file executes top-level statements. There is no module initialization order.
- Names are scoped to their declaring module. Duplicate declarations and aliases,
  including collisions between aliases and top-level declarations, are errors.
  Existing restrictions on built-in names continue to apply.
- Each canonical source file is loaded once, even through different relative
  paths or a diamond dependency. Circular imports, including self-imports, are
  errors; diagnostics show the cycle.
- Record identity includes its declaring module. Equal-looking records in
  different modules are distinct types. An imported function may use its own
  module's private helper functions and private record types internally.
- Every reachable source file is validated before code generation. Functions and
  records are lowered into one LLVM module and linked into one executable.
- Source-only compilation without a named file cannot resolve relative imports;
  use the file-based compiler API for a multi-file program.

Separate object files, incremental builds, packages, re-exports, exported
variables, and executable library initialization are outside this first version.

## Diagnostics and compiler APIs

Diagnostics in dependencies show the original file, line, source excerpt, and
import chain. Circular import errors show the full cycle. Runtime safety errors
in multi-file programs identify the failing source file and local byte range.
Debug builds emit a source file entry for each dependency.

`compile_file` and `compile_file_with_options` discover dependencies automatically.
`compile_named_source_with_options` verifies the entry source against disk and
uses the same module pipeline. Unnamed `compile_source` APIs accept standalone
modules but report a diagnostic for imports because no base directory is known.

Existing single-file programs do not need a module declaration or exports.
New keywords `module`, `import`, and `export` are reserved. There is no implicit
exporting of existing library declarations.

See [the runnable example](examples/modules/main.chuchu), which prints `5`, `3`,
and `5` on separate lines and shares its `math` dependency with `geometry`.
