# Bash ISE

A small PowerShell-ISE-style environment for bash scripts: a tabbed script editor on top, a live console underneath. Written in Rust with [eframe/egui](https://github.com/emilk/egui).

![Bash ISE running a script](bash-ise-screenshot.png)

## Features

**Editor**
- Bash syntax highlighting (quotes, `$variables`, `$(command substitution)`, heredocs, `case` patterns)
- Line numbers that stay in place when scrolling sideways, current-line highlight
- Auto-indent on Enter
- Find with match-case option
- Snippets for common constructs (`if`, `for`, `while read`, `case`, function, strict mode)

**Files**
- Multiple tabs, with a dot on tabs that have unsaved changes
- New, Open, Save, Save As; drag and drop from the file manager
- Open files from the command line: `bash-ise script.sh other.sh`
- Prompt before closing a tab or the window with unsaved changes

**Running**
- Run the whole script, with optional arguments from the toolbar
- Run only the selection, or the current line if nothing is selected
- Stop a running script together with its child processes

**Console**
- Output appears live while the script runs; stderr is shown in red
- Exit code and duration after every run
- Command line with history; `cd` is remembered between commands
- While a script runs, the input line feeds its stdin, so `read` prompts work
- ANSI colour codes are stripped, `\r` progress bars update in place
- Keeps the last 20 000 lines

**Look**
- Dark and light theme (or follow the system), under View
- Light theme uses the PowerShell ISE colours; the console is ISE navy in both
- Zoom in and out

## Requirements

- Linux or another Unix (Stop relies on process groups and signals)
- `bash` on the `PATH`
- Rust 1.95 or newer (required by eframe 0.37)
- A running `xdg-desktop-portal` for the Open / Save dialogs (standard on KDE and GNOME)

## Build and run

```sh
cargo run --release              # start with an empty script
cargo run --release -- deploy.sh # open one or more files
```

The binary ends up in `target/release/bash-ise`.

## Keyboard shortcuts

| Action | Shortcut |
| --- | --- |
| New script | Ctrl+N |
| Open | Ctrl+O |
| Save | Ctrl+S |
| Save As | Ctrl+Shift+S |
| Close tab | Ctrl+W (or middle-click the tab) |
| Quit | Ctrl+Q |
| Find | Ctrl+F |
| Find next / previous | F3 / Shift+F3 (Enter / Shift+Enter in the find bar) |
| Run script | F5 |
| Run selection or current line | F8 |
| Stop | Shift+F5 (press again to force-kill) |
| Clear console | Ctrl+L |
| Zoom in / out / reset | Ctrl+Plus / Ctrl+Minus / Ctrl+0 |

In the console input line:

| Action | Key |
| --- | --- |
| Run the command, or send the line to the running script | Enter |
| Previous / next command | Up / Down |
| End of input for the running script | Ctrl+D |
| Stop the running script (when the input line is empty) | Ctrl+C |
| Clear the console | type `clear` or `cls` |

## How scripts are run

- **F5** runs `bash <file> <arguments>`. A saved file is saved first; an untitled script is run from a temporary copy that is deleted afterwards. Arguments are split by bash itself, so quotes and `$VARS` behave as on a command line.
- **F8** and the console input run the text with `bash -c`.
- Everything starts in the working directory shown in the status bar. Change it with `cd` in the console.
- **Stop** sends SIGTERM to the script's process group; a second Stop sends SIGKILL.

## Limitations

- Every run is a new bash process. Variables, functions and aliases do not carry over between runs or console commands; only the working directory does.
- There is no terminal emulation. Programs that need a real TTY (`sudo` password prompts, `vim`, `top`) do not work in the console.
- stdout and stderr are read through separate pipes, so an error line can appear a line or two away from where it was printed when both streams are written at the same moment.
- Programs that buffer their output when not attached to a terminal may show it in bursts.
- No debugger or breakpoints.
- The highlighter is a tokenizer, not a full bash parser; unusual constructs may be coloured imperfectly.

## Code layout

Everything is in `src/main.rs`, in six sections:

| Section | Contents |
| --- | --- |
| 1. Theme | Colours and widget styling for both themes |
| 2. Highlighter | Bash tokenizer that produces the coloured editor text |
| 3. Console | Line buffer behind the output pane |
| 4. Runner | Starts bash on background threads and streams its output over a channel |
| 5. Tabs | One open document per tab |
| 6. App | State, user actions and the UI |

## Tests

```sh
cargo test
```

The tests cover the parts that need no window: the highlighter, the console buffer, ANSI stripping and UTF-8 handling of process output.

## Dependencies

- [`eframe`](https://crates.io/crates/eframe) for the window and UI
- [`rfd`](https://crates.io/crates/rfd) for native file dialogs
- [`libc`](https://crates.io/crates/libc) for sending signals to the script's process group
