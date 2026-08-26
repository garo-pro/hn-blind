# hn-blind

An accessible Hacker News client in Rust, built for screen reader users.

The interface is a real native window, built with
[wxDragon](https://github.com/AllenDang/wxDragon) — Rust bindings to
wxWidgets. A list control holds the story and help lists, a tree control the
comment threads, and there is a menu bar and a dialog for settings. A screen
reader already knows how to read all of those, because they are the same
controls every other application on the platform uses; nothing here has to
describe itself to the accessibility API by hand.

Alongside them, direct speech and braille output goes through
[Prism](https://github.com/garo-pro/prism2rust), for everything a focused
control cannot say on its own. Everything is keyboard driven.

## Requirements

- Rust 1.85 or newer (the crate is on the 2024 edition).
- **A C++ toolchain, CMake, and libclang.** Two dependencies build native
  code from source: `prism-sys` compiles the vendored Prism library, which
  wants C++23, and `wxdragon-sys` downloads and builds wxWidgets itself,
  generating its bindings with `bindgen`. On Windows, Visual Studio 2022 with
  the C++ workload plus CMake and LLVM satisfies all of it; on Linux, GCC 13+
  or Clang 17+, `cmake`, `libclang-dev`, and the GTK 3 development packages.
- The first build is a long one, because it is building a GUI toolkit.
  Later builds reuse it.

## Build and run

```sh
cargo run --release
```

To check the API client and the wording of announcements without opening a
window:

```sh
cargo run --example preview -- top 10
```

To list every phrase the application can say, with its placeholders and
whichever of them you have changed:

```sh
cargo run --example preview -- templates
```

## Keys

| Key | Action |
| --- | --- |
| Up / Down, J / K | Move through the list |
| Page Up / Page Down | Move ten rows |
| Home / End | First or last row |
| Enter | Open the selected story's comments |
| O | Open the story link in your browser |
| C | Open the Hacker News discussion page |
| Backspace / Escape | Back to the story list |
| R | Reload the current feed |
| 1 – 6 | Top, New, Best, Ask, Show, Jobs |
| P | Read the selected item in full |
| S | Stop speaking |
| V | Toggle Prism speech |
| Alt | Open the menu bar, a second way to reach every command |
| , | Settings |
| H / F1 | Keyboard help |
| Q | Quit |

The key map is also the in-app help view (`H`), so it is discoverable without
reading this file. Every command is also on the menu bar, for anyone who does
not already know the letters by heart; the menu and the key run the same
code, so the two cannot drift apart.

Whether Escape quits from the story list, rather than saying there is nowhere
further back to go, is a setting — see the General tab.

## How the two output channels divide the work

Using the platform's own accessibility support and Prism together risks
announcing everything twice. They are kept strictly disjoint:

- **The native controls** own *what is focused*. A feed is a single-column
  `wxListCtrl` whose rows are the story labels; a comment thread is a
  `wxTreeCtrl` whose nesting is the reply depth, so a screen reader announces
  the level natively; the settings dialog is a real modal dialog with a
  notebook, a tree and an edit field. wxWidgets publishes all of this to
  MSAA/UIA (and to AT-SPI on Linux, NSAccessibility on macOS) without being
  asked.
- **Prism** owns *transient status* — load progress, errors, feed changes —
  and on-demand reading of the full text of an item (`P`), which is far too
  long to sit in a row label.

Two details make this adapt to the user's setup rather than assuming one:

- Prism reports which backend it acquired. If that backend is a screen reader
  (NVDA, JAWS, VoiceOver, Orca…), it is already announcing the focused row and
  this app stays quiet on movement. If it is a bare TTS engine (SAPI, OneCore,
  AVSpeech…), nothing else is speaking, so focus changes — rows, menu items,
  settings fields — are announced here instead.
- Status goes to the window's status bar as well as to Prism. A screen reader
  with native support for the standard status bar control announces its
  changes on its own when Prism is not speaking, so exactly one channel says
  it either way.

Where a backend advertises `SUPPORTS_OUTPUT`, output is sent through Prism's
`output` rather than `speak`, so it reaches a braille display as well as speech.

## Everything it says is a template

No announcement is assembled by string concatenation in the code. Every phrase
— row labels, list titles, status messages, error messages, the ages on stories,
even the word for a story with no title — is a named template you can edit. What
one listener wants on every row (domain, score, author, age, comment count) is
padding to the next one, and neither of them necessarily wants it in English.

Templates use three pieces of syntax:

| Syntax | Meaning |
| --- | --- |
| `{name}` | A value. Which names a template accepts is listed against it |
| `[...]` | An optional group: dropped entirely if a placeholder inside it is empty |
| `\n`, `\[`, `\\` | A line break; a literal character |

The optional group is what makes one template cover a story that has a link, a
score and an author and one that has none of them, without a trail of stray
commas for the synthesizer to read:

```
{index}. {title}[, {domain}][, {score} points][, by {author}], {age}, {comments}
```

Rendering never fails. An unbalanced bracket produces its own text rather than
an error, because a user halfway through an edit still needs the application to
talk to them; the problem is reported when the settings dialog is closed.

Edits are written to `%APPDATA%\hn-blind\templates.json` (on Linux,
`$XDG_CONFIG_HOME/hn-blind/`; on macOS, `~/Library/Application Support/`), and
only the templates you actually changed are stored, so improvements to the
default wording still reach you. The yes/no preferences live beside them in
`preferences.json`.

## The settings dialog

`,` opens a real modal dialog: a notebook of tabs, and in each tab a tree of
the fields on the left next to the editor for whichever one is selected. The
Templates tab holds one field per phrase; the General tab holds the switches
that are not phrases at all, of which there is currently one — whether Escape
quits from the story list.

Everything in it is the platform's own dialog navigation, which is the point
of building it out of real controls:

| Key | Action |
| --- | --- |
| Up / Down | Move through the fields |
| Left / Right | Collapse or open a group |
| Tab / Shift+Tab | Move between the field list and the editor |
| Ctrl+Tab / Ctrl+Shift+Tab | Switch tabs |
| F5 | Restore the selected field to its default |
| F1 | The keys, again |
| Escape | Close, saving |

The fields are grouped — story list, comments, help, status, times, individual
words — because a screen reader announces entering and leaving a group, and
seventy fields in a flat list is a wall rather than a list.

The editor itself is an ordinary multi-line text control, so the caret, the
selection, and the screen reader's keyboard echo all behave exactly as they do
in every other application. What the dialog adds on top is the description
beside each field — its placeholders, and whether you have changed it — and,
with only a bare TTS engine attached, spoken announcements of the field and
tab you have moved to.

## Layout

| File | Role |
| --- | --- |
| `src/hn.rs` | Firebase API client; parallel batch fetches, comment threads flattened into reading order |
| `src/html.rs` | Converts HN's HTML comment bodies into plain text fit for speech |
| `src/templates.rs` | Every phrase the application can say, and the renderer for them |
| `src/config.rs` | Loading and saving the templates the user has changed |
| `src/preferences.rs` | The same, for the yes/no switches that are not phrases |
| `src/settings.rs` | The settings dialog's tabs, fields and grouping |
| `src/menu.rs` | Which command sits under which menu entry, and its stable id |
| `src/app.rs` | Application state, and which template describes it |
| `src/speech.rs` | Prism backend lifecycle and the screen-reader/TTS distinction |
| `src/main.rs` | Every widget: the window, its controls, the key map, the menu bar, the settings dialog, and the network worker thread |

Only `main.rs` links against wxWidgets. Everything else is plain data and
wording, which is why the API client, the HTML conversion and every phrase
this application can say have unit tests and the window does not need one.

Network work runs on a worker thread and reports its results back through a
channel, which the GUI thread drains from an idle handler — the worker wakes
that loop with the one wx call that is safe from another thread. Each request
carries a generation number, so replies for navigation the user has already
moved on from are discarded rather than overwriting what they are currently
reading.

## Notes and limits

- Feeds load the first 50 stories; a comment thread loads up to 400 comments,
  fetched a level at a time so each level's requests run in parallel.
- Read-only. There is no login, voting, or posting.
- The window mirrors the current position in its title bar and the latest
  status message in its status bar, so a sighted person looking over your
  shoulder can follow along.
