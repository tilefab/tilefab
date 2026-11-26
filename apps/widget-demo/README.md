# widget-demo

Demo of the [tilefab](https://crates.io/crates/tilefab) TUI library: log lines scroll above an
**inline viewport** that shows one progress bar per running task, with an input prompt on its last
line. The viewport grows and shrinks as tasks start and finish.

```
> start beta
[INFO] task "alpha" started (duration 6.3 s)
[INFO] task "beta" started (duration 5.5 s)
alpha [05.123] 🟠 ███████████████░░░░░░░░░░░░░░░░░░░░░░░░░░  36% compiling
beta  [05.123] 🟠 ██████████████████░░░░░░░░░░░░░░░░░░░░░░░  42% compiling

> start gam█
```

## Install and run

Not on crates.io yet: install it from a clone of the repository.

```sh
git clone https://github.com/tilefab/tilefab
cd tilefab
cargo install --path apps/widget-demo
widget-demo
```

Commands: `start <name> ["<composition>"]`, `list`, `compose ["<composition>"]`, `help`, `quit`
(or `exit`, Ctrl+C, Ctrl+D). A composition describes the row of a task, for instance
`"N B0 B4 T2 B5 B0 S B0 I B0 P B0 L"`; `help` lists its tokens, the looks of the bar and the
paints.
Outside a terminal (stdin or stdout redirected) it reads commands line by line from stdin:
`printf 'start a\nstart b\n' | widget-demo`.

More details in the [repository README](https://github.com/tilefab/tilefab#readme).

License: MIT, see [LICENSE](https://github.com/tilefab/tilefab/blob/main/LICENSE).
