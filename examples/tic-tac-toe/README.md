# Chuchu Tic-Tac-Toe (`examples/tic-tac-toe`)

A complete, native console **Tic-Tac-Toe** game written **100% in Chuchu**, showcasing:
- **Multi-module architecture** (`module`, `export`, `use module`)
- **Record types & value semantics** (`board.Board`, `stats.Scoreboard`, `ui.InputChoice`)
- **Recursive Minimax AI** (`ai.chuchu`) leveraging safe value-copied board states
- **Checked I/O & error handling** (`try_input`, `parse_int`, `try_read_file`, `write_file`)

## Module Structure

| File | Description |
| :--- | :--- |
| `main.chuchu` | Application entry point, menu loop, and turn controller |
| `board.chuchu` | `Board` record definition, cell lookup, validation, and move mutation |
| `rules.chuchu` | Row/column/diagonal win detection and draw evaluation |
| `ai.chuchu` | Easy tactical AI and unbeatable recursive Minimax AI |
| `ui.chuchu` | ASCII 3×3 grid renderer, prompts, and fault-tolerant input parser |
| `stats.chuchu` | Session `Scoreboard` record and `scoreboard.txt` file persistence |

## Build & Run

From the repository root (`chuchu-core`):

```bash
# Compile to a native executable
cargo run --bin cheeku -- examples/tic-tac-toe/main.chuchu -o tic-tac-toe

# Run the game
./tic-tac-toe
```
