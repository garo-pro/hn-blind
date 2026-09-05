# hn-blind

An accessible Hacker News client in Rust, built for screen reader users. The accessibility *is* the product: keyboard navigation and speech output are the deliverable, not a layer over a visual UI.

## Prose style in this repo

Comments, doc comments and Markdown are written **one line per paragraph**, with no hard wrapping — the editor wraps, the file does not. A reflow to this style was done deliberately (commit 54518fa); re-wrapping a paragraph at 80 columns produces a diff that fights every other line in the file.

Comments explain *why*, not *what*, and are allowed to be long where the reasoning is load-bearing. `src/templates.rs` and `src/speech.rs` set the tone. A comment that restates the code it sits on is worse than no comment.

## Two output channels, never both at once

The native wxDragon/wxWidgets controls own **focus** announcements: a list control for stories and help, a tree control for comments, plus a menu bar and settings dialog. A screen reader reads those on its own, because they are the same controls every other application uses.

Prism (`src/speech.rs`) owns everything a focused control cannot say: load progress, errors, feed changes, and the explicit "read this to me now" key (`P`).

`Speaker::announces_focus()` is true only when Prism got a bare TTS engine rather than a screen reader. Any new user-visible event needs a decision about which channel announces it — adding a Prism call next to a control update without checking that flag makes a screen reader say everything twice, which is worse than either channel alone.

## Every spoken phrase is a template

No announcement is built by string concatenation. Each one is a named, user-editable template in `src/templates.rs`, edited in the settings dialog and persisted by `src/config.rs`. What is sensible padding to one listener is intolerable to another, so the wording belongs to the user. New user-facing text means a new template, not a `format!` at the call site.

## Building

`cargo check` for ordinary iteration. Avoid casual full builds: `wxdragon-sys` downloads and builds wxWidgets from source and `prism-sys` compiles vendored C++23, so a cold build is very long. Later builds reuse it.

Release builds link Prism statically (`PRISM_STATIC=1`), which is a materially different link path from a normal build and has broken on its own before — CI covers it in the `static-link` job, so run that way before touching `build.rs` or the link configuration.

Shell scripts are pinned to LF in `.gitattributes`; this repo is developed on Windows and a CRLF shebang breaks the Linux CI runners.
