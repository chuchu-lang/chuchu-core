# Chuchu Calculator

An interactive terminal calculator written entirely in the Chuchu language. It has
a menu-driven interface, validates every answer, and keeps a history of the
calculations made in the current session.

## Features

- Addition, subtraction, multiplication and division
- Integer, decimal and negative numbers (`12`, `-3.5`, `.25`, `1e3`)
- Clean results: `0.1 + 0.2` shows `0.3`, not `0.30000000000000004`
- Main menu, with options to repeat a calculation or return to the menu
- In-session calculation history, with the option to clear it
- Input validation with clear error messages and automatic retry:
  - non-numeric or empty numbers
  - invalid menu choices
  - division by zero (asks for a different second number)
- Type `quit` at any prompt, press Ctrl+D (end of input), or choose `[0] Exit` to
  leave. All of these end the program normally with exit status 0.

## Directory structure

```text
calculator/
├── calculator.chuchu   the program (about 400 lines of Chuchu)
└── README.md           this file
```

## How to run

From the `chuchu-core` folder, compile the program and run it:

```bash
cargo run --release -- examples/projects/calculator/calculator.chuchu -o /tmp/calculator
/tmp/calculator
```

Building the compiler (`cheeku`) needs the project's LLVM SDK. If `cargo` reports
that no suitable version of LLVM was found, point it at the SDK first:

```bash
export LLVM_SYS_231_PREFIX=$PWD/target/llvm-sdk/LLVM-23.1.4-Linux-X64
export PATH=$LLVM_SYS_231_PREFIX/bin:$PATH
export LD_LIBRARY_PATH=$LLVM_SYS_231_PREFIX/lib:$LD_LIBRARY_PATH
```

Once `cheeku` is built, the compiled calculator runs without any of these
variables set.

## Example session

Lines after a prompt are what the user typed.

```text
========================================
           CHUCHU CALCULATOR
========================================
        Simple. Fast. Powerful.

               MAIN MENU
----------------------------------------
  [1] Addition
  [2] Subtraction
  [3] Multiplication
  [4] Division
  [5] Calculation History
  [0] Exit
----------------------------------------
Type 'quit' at any prompt to leave.

Select an option: 1

========================================
                ADDITION
========================================

Enter first number: 23
Enter second number: 12

----------------------------------------
  23 + 12 = 35
----------------------------------------

[1] New Calculation
[2] Main Menu
[0] Exit

Select an option: 2
```

After a few calculations, option `[5]` on the main menu shows the history:

```text
========================================
          CALCULATION HISTORY
========================================

  1. 23 + 12 = 35
  2. 11 * 22 = 242
  3. 50 / 2 = 25

----------------------------------------
[1] Clear History
[2] Main Menu
[0] Exit

Select an option:
```

Errors explain the problem and ask again:

```text
Enter first number: abc
  Error: 'abc' is not a number. Try 12, -3.5 or 0.25.
Enter first number: 5
Enter second number: 0
  Error: Division by zero is undefined. Enter a different second number.
Enter second number: 2
```

## How the code is organised

Chuchu functions cannot read top-level variables and cannot be called as
statements, so the program is built in three layers:

1. **Text builders** (`heading`, `main_menu_text`, `operation_title`, ...) return a
   string, and the main loop prints it with `show`.
2. **Input helpers** (`ask_choice`, `ask_number`) read one answer, validate it and
   retry until it is valid. `ask_number` returns a `result<float>`; `ask_choice`
   returns the selected number, or `-1` for `quit` and end of input.
3. **Calculation and formatting** (`calculate`, `format_number`, `describe`) hold the
   arithmetic and have no input or output of their own.

The main loop at the bottom is a small state machine. The variable `screen` is 0
for the main menu, 1 for a calculation, 2 for the menu after a calculation, 3 for
the history and 4 to leave.

## Current limitations

- **History is one block of text, not a list.** Chuchu arrays only hold
  `int` or `float` values (at most 256), so they cannot store the text of a
  calculation. The history is therefore one string with a numbered line per
  calculation, plus a counter. It has no fixed size limit other than available
  memory, it cannot be edited entry by entry, and it is lost when the program ends.
- **Numbers are 64-bit floats.** Whole numbers above 2^53 (about 9 quadrillion)
  lose exactness. Results are *displayed* rounded to 8 decimal places (4 for values
  of 10,000,000 or more, 16 for values below 0.0000001), without trailing zeros.
  Values outside about 0.000000000001 to 100 billion are shown by the language's
  default conversion, for example `1e+20` or `1e-13`. The calculation itself keeps
  full precision.
- **Only four operations.** There is no remainder, power, square root, or use of the
  previous answer, and the language has no `%` operator.
- **`quit` is exact.** It must be lowercase with no surrounding spaces. Menu
  choices and numbers may have surrounding spaces, and a leading `+` is accepted.
- **A failed division is not added to the history.**
- **The calculator never clears the screen.** Each screen is printed below the last.
