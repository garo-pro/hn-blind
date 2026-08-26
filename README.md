# hn-blind

An accessible Hacker News client in Rust, built for screen reader users.

There is no drawn interface. The window is a blank surface; the actual user
interface is an [AccessKit](https://accesskit.dev) tree published straight to
the platform accessibility API, plus direct speech and braille output through
[Prism](https://github.com/garo-pro/prism2rust). Everything is keyboard driven.

## Requirements

- Rust 1.82 or newer.
- **A C++23 compiler and CMake.** `prism-sys` builds the native Prism library
  from a vendored C++ source tree. On Windows, Visual Studio 2022 with the C++
  workload satisfies this; on Linux, GCC 13+ or Clang 17+.

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
| , | Settings |
| H / F1 | Keyboard help |
| Q | Quit |

The key map is also the in-app help view (`H`), so it is discoverable without
reading this file.

## How the two output channels divide the work

Using AccessKit and Prism together risks announcing everything twice. They are
kept strictly disjoint:

- **AccessKit** owns *what is focused*. Story feeds are exposed as a `ListBox`
  of `ListBoxOption`s; a comment thread is a `Tree` of `TreeItem`s carrying
  `level`, so a screen reader announces reply depth natively. Only the selected
  row sets the `selected` flag, which avoids a "not selected" announcement on
  every other row.
- **Prism** owns *transient status* — load progress, errors, feed changes — and
  on-demand reading of the full text of an item (`P`), which is too long to sit
  in a node label.

Two details make this adapt to the user's setup rather than assuming one:

- Prism reports which backend it acquired. If that backend is a screen reader
  (NVDA, JAWS, VoiceOver, Orca…), AccessKit is already announcing focus and this
  app stays quiet on movement. If it is a bare TTS engine (SAPI, OneCore,
  AVSpeech…), nothing else is speaking, so focus changes are announced here.
- The status node is marked as a `Polite` live region **only** when Prism speech
  is off. Exactly one channel announces status at any time.

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
default wording still reach you.

## The settings dialog

`,` opens a dialog with no pixels: a tab list and one edit field per template,
published to the accessibility tree and driven from the keyboard like any
platform dialog. There is one tab, Templates; the tab strip is there because
the second tab should be a line of code, not a rewrite.

| Key | Action |
| --- | --- |
| Tab / Shift+Tab, Up / Down | Move between the tab strip and the fields |
| Ctrl+Down / Ctrl+Up | Jump to the next or previous group of templates |
| Page Up / Page Down | Move ten fields |
| Left / Right, Home / End | Move through the text of a field |
| Left / Right on the tab strip | Switch tabs, as does Ctrl+Tab |
| F5 | Restore this field to its default |
| F1 | The keys, again |
| Escape | Close, saving |

The fields are grouped — story list, comments, help, status, times, individual
words — because a screen reader announces entering and leaving a group, and
seventy fields in a flat list is a wall rather than a list.

The editor is a single line with a caret and no selection: a selection no screen
reader can see is a control the user cannot hear. Caret movement and deletions
are spoken through the same transient channel as everything else. Typed
characters are not, because the screen reader's own keyboard echo already says
them — unless Prism has only a bare TTS engine, in which case nothing else is
listening to the keyboard and this application echoes.

## Layout

| File | Role |
| --- | --- |
| `src/hn.rs` | Firebase API client; parallel batch fetches, comment threads flattened into reading order |
| `src/html.rs` | Converts HN's HTML comment bodies into plain text fit for speech |
| `src/templates.rs` | Every phrase the application can say, and the renderer for them |
| `src/config.rs` | Loading and saving the templates the user has changed |
| `src/settings.rs` | The settings dialog's tabs, focus ring and line editor |
| `src/app.rs` | Application state and its projection into an AccessKit `TreeUpdate` |
| `src/speech.rs` | Prism backend lifecycle and the screen-reader/TTS distinction |
| `src/main.rs` | winit event loop, key handling, network worker thread |

Network work runs on a worker thread and reports back through the winit event
loop proxy. Each request carries a generation number, so replies for navigation
the user has already moved on from are discarded rather than overwriting what
they are currently reading.

## Notes and limits

- Feeds load the first 50 stories; a comment thread loads up to 400 comments,
  fetched a level at a time so each level's requests run in parallel.
- Read-only. There is no login, voting, or posting.
- The window paints a flat colour and mirrors the current position in its title
  bar, which is the only thing a sighted person looking over your shoulder can
  read.
