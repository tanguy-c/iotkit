---
name: size-check
description: Compare the ESP32-C6 firmware sizes (devkit, devkit + matter-wifi, thread-light) against a base revision. Use before committing any change that could reach the C6 build.
---

1. Run `tools/size-check.sh [base-rev]` (default `origin/main`, else `HEAD~`). It builds the base in
   a worktree under `target/size-check/base` and the working tree, sharing `target/`. The first
   run is slow (two release builds of three configurations).
2. Read the output per build: section sizes with deltas, then the ten largest symbol deltas.
3. Growth of `.text`, `.rwtext*`, `.data*` or `.bss` fails (exit 1). For a change meant for
   another chip or an opt-in feature, any C6 delta is a bug: put the new code behind its feature
   or `cfg` (see AGENTS.md, Rules).
4. Growth that is the point of the change (a C6 feature, a fix): say so in the commit message;
   `ALLOW_GROWTH=1` locally, the `size-ok` label on the PR.
5. `.rodata` changes alone (panic line numbers, strings) are reported but do not fail.
6. Then `tools/link-check.sh`: unused code must not be linked. A new opt-in feature or component
   adds a rule (pattern, a build without it, a control build with it).
