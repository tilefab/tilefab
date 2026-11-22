# tilefab-demo

Demo of the [tilefab](https://crates.io/crates/tilefab) TUI library: log lines scroll above an
**inline viewport** that shows one progress bar per running task, with an input prompt on its last
line. The viewport grows and shrinks as tasks start and finish.

```
> start beta
[INFO] task "alpha" started (duration 6.3 s)
[INFO] task "beta" started (duration 5.5 s)
alpha      ████████████░░░░░░░░░░░░░░░░░░░░░  36%
beta       ██████████████░░░░░░░░░░░░░░░░░░░  42%

> start gam█
```

## Install and run

```sh
cargo install tilefab-demo
tilefab-demo
```

Type `help` at the prompt for the list of commands (`start <name>`, `list`, `quit`…).
Outside a terminal (stdin or stdout redirected) it reads commands line by line from stdin:
`printf 'start a\nstart b\n' | tilefab-demo`.

More details in the [repository README](https://github.com/tilefab/tilefab#readme).

License: MIT, see [LICENSE](https://github.com/tilefab/tilefab/blob/main/LICENSE).
