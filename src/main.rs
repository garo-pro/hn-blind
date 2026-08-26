//! hn-blind — an accessible Hacker News client.
//!
//! The interface is a real native wxWidgets window: a `ListCtrl` for the story and help lists, a `TreeCtrl` for comment threads, a native `MenuBar`, and a `Dialog` for settings. wxWidgets' own MSAA/UIA support is what a screen reader reads; Prism supplies direct speech for status and on-demand full reading. See `app.rs` for how state becomes wording, `speech.rs` for how the two output channels stay out of each other's way, and `templates.rs` for where the wording itself comes from.

// No console window when launched normally (e.g. double-click); `cargo run` still shows one because it launches through a terminal already.
#![windows_subsystem = "windows"]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use wxdragon::event::{IdleEvent, IdleMode, TextEventData, TreeEventData, WindowEvents};
use wxdragon::menus::menu::MenuBuilder;
use wxdragon::keycode::{
    WXK_DOWN, WXK_END, WXK_ESCAPE, WXK_F1, WXK_F5, WXK_HOME, WXK_LEFT, WXK_PAGEDOWN, WXK_PAGEUP, WXK_RIGHT, WXK_UP,
};
use wxdragon::prelude::*;
use wxdragon::widgets::checkbox::CheckBoxEventData;
use wxdragon::widgets::item_data::HasItemData;
use wxdragon::widgets::list_ctrl::ListCtrlEventData;
use wxdragon::widgets::notebook::NotebookPageChangedEvent;

use hn_blind::app::{App, View};
use hn_blind::config;
use hn_blind::hn::{self, CommentRow, Feed, Item};
use hn_blind::menu::{self, Command, Item as MenuEntry, Kind};
use hn_blind::preferences;
use hn_blind::settings::{Field, TABS, fields_of, groups};
use hn_blind::speech::Speaker;
use hn_blind::templates::{Template, validate};

/// How many stories to pull per feed. HN's lists run to 500; a few screens' worth is what anyone actually reads.
const STORY_LIMIT: usize = 50;
/// Upper bound on comments fetched for one story, to keep big threads snappy.
const COMMENT_LIMIT: usize = 400;
/// Rows moved by Page Up / Page Down.
const PAGE: isize = 10;

/// Work handed to the network thread.
enum Request {
    Stories { generation: u64, feed: Feed },
    Comments { generation: u64, story: Box<Item> },
}

/// Results handed back from the network thread. Plain, `Send` data only — the actual `Gui` state lives behind an `Rc`, which cannot cross threads, so a background thread only ever produces one of these and never touches the UI directly. See `apply_result` for where it lands.
enum WorkResult {
    Stories {
        generation: u64,
        feed: Feed,
        result: Result<Vec<Item>, String>,
    },
    Comments {
        generation: u64,
        result: Result<Vec<CommentRow>, String>,
    },
}

/// Everything that isn't a widget handle.
struct AppState {
    app: App,
    speaker: Speaker,
    generation: u64,
    requests: mpsc::Sender<Request>,
    /// Parallel to `app.comments`: the tree node for each row, so a cursor index can be turned into something `TreeCtrl` understands.
    comment_items: Vec<TreeItemId>,
}

/// The application's shared handle. Widgets are cheap `Copy` types, so they live here directly; only `AppState` needs a `RefCell`. Cloning a `Gui` is cheap and is how every closure gets its own handle to the whole app.
#[derive(Clone)]
struct Gui {
    state: Rc<RefCell<AppState>>,
    /// Set for the duration of a *programmatic* selection change on `list` or `tree`, so the resulting native focus/selection event (which some platforms fire synchronously) knows to ignore an echo of a move this process already announced, rather than re-entering `state`'s `RefCell` while it may still be borrowed.
    suppress: Rc<Cell<bool>>,
    frame: Frame,
    /// The frame's content area, holding `list` and `tree`. Kept so that hiding one and showing the other can be followed by a re-layout, which is what hands the whole area to whichever one is now visible.
    panel: Panel,
    list: ListCtrl,
    tree: TreeCtrl,
    /// Behind an `Rc` only because `MenuBar`, alone among the handles here, is not `Clone`: the frame owns the real menu bar and this is a borrowed view of it.
    menu_bar: Rc<MenuBar>,
}

/// `ListCtrl` is the one control here that wxdragon does not give the `WindowEvents` category to, so it cannot be passed to `bind_content_keys` beside `TreeCtrl`. Every method of that trait is a default over `WxEvtHandler`, so a local wrapper earns them all back — cheaper than a second copy of the key handling for the sake of one widget.
struct KeyTarget(ListCtrl);

impl WxEvtHandler for KeyTarget {
    unsafe fn get_event_handler_ptr(&self) -> *mut wxdragon::ffi::wxd_EvtHandler_t {
        unsafe { self.0.get_event_handler_ptr() }
    }
}

impl WindowEvents for KeyTarget {}

fn main() {
    SystemOptions::set_option_by_int("msw.no-manifest-check", 1);
    let _ = wxdragon::main(|_| run());
}

fn run() {
    let (templates, templates_note) = config::load();
    let (preferences, preferences_note) = preferences::load();
    let startup_note = match (templates_note, preferences_note) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (Some(note), None) | (None, Some(note)) => Some(note),
        (None, None) => None,
    };

    let frame = Frame::builder()
        .with_title("hn-blind")
        .with_size(Size::new(900, 600))
        .build();
    frame.set_extra_style(ExtraWindowStyle::ProcessIdle);
    IdleEvent::set_mode(IdleMode::ProcessSpecified);

    let panel = Panel::builder(&frame).build();
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    // Report mode with a single unnamed column: one row is one announcement, and the row label carries everything. `SingleSel` because the cursor this application tracks is a single row — a range selection would be something it could not describe. `NoHeader` because a column with no name has no header worth landing on.
    let list = ListCtrl::builder(&panel)
        .with_style(ListCtrlStyle::Report | ListCtrlStyle::SingleSel | ListCtrlStyle::NoHeader)
        .build();
    list.insert_column(0, "", ListColumnFormat::Left, 860);
    let tree = TreeCtrl::builder(&panel)
        .with_style(TreeCtrlStyle::HasButtons | TreeCtrlStyle::LinesAtRoot | TreeCtrlStyle::HideRoot)
        .build();
    tree.show(false);

    sizer.add(&list, 1, SizerFlag::Expand | SizerFlag::All, 0);
    sizer.add(&tree, 1, SizerFlag::Expand | SizerFlag::All, 0);
    panel.set_sizer(sizer, true);

    let _status_bar = frame.create_status_bar(1, 0, -1, "");

    let (requests, worker_rx) = spawn_worker();

    let app = App::new(Feed::Top, templates, preferences);
    let speaker = Speaker::new();
    let state = Rc::new(RefCell::new(AppState {
        app,
        speaker,
        generation: 0,
        requests,
        comment_items: Vec::new(),
    }));

    let menu_bar = build_menu_bar(&state.borrow().app);
    frame.set_menu_bar(menu_bar);
    let menu_bar = Rc::new(frame.get_menu_bar().expect("menu bar was just set"));

    let gui = Gui {
        state,
        suppress: Rc::new(Cell::new(false)),
        frame,
        panel,
        list,
        tree,
        menu_bar,
    };

    bind_menu_events(&gui);
    bind_content_keys(&KeyTarget(gui.list), &gui);
    bind_content_keys(&gui.tree, &gui);
    bind_content_selection(&gui);
    bind_context_menu(&gui);
    bind_worker_idle(&gui, worker_rx);

    sync_feed_labels(&gui);
    sync_menu_checks(&gui);

    gui.frame.show(true);
    gui.frame.centre();

    let feed = gui.state.borrow().app.feed;
    load_feed(&gui, feed);

    // Said after the request is in flight, so it replaces "Loading" rather than being cut off by it. The feed announces itself when it arrives.
    if let Some(note) = startup_note {
        set_status(&gui, note);
    }
}

// ---- Wording / title -------------------------------------------------------

/// Mirror position into the window title, the one thing a sighted person looking over a shoulder can read.
fn sync_title(gui: &Gui) {
    let s = gui.state.borrow();
    let count = s.app.row_count();
    let (position, total) = if count == 0 {
        (String::new(), String::new())
    } else {
        ((s.app.position() + 1).to_string(), count.to_string())
    };
    let title = s.app.text(
        Template::WindowTitle,
        &[
            ("title", &s.app.list_title()),
            ("position", &position),
            ("count", &total),
        ],
    );
    drop(s);
    gui.frame.set_title(&title);
}

/// Set the status line and speak it.
///
/// Status is Prism's job. A screen reader with native support for the standard Windows status bar control announces its text changes on its own when Prism itself is not speaking; either way exactly one channel says it.
fn set_status(gui: &Gui, text: impl Into<String>) {
    let text = text.into();
    gui.state.borrow_mut().app.status = text.clone();
    gui.frame.set_status_text(&text, 0);
    gui.state.borrow_mut().speaker.announce(&text);
}

/// Announce a change of view: the new context, then the row now focused.
///
/// Both go out as a single utterance because `announce` interrupts, so two calls would leave the user hearing only the second.
fn enter_view(gui: &Gui, status: impl Into<String>) {
    gui.state.borrow_mut().app.status = status.into();
    populate_view(gui);
    sync_title(gui);

    let s = gui.state.borrow();
    gui.frame.set_status_text(&s.app.status.clone(), 0);
    let label = if s.speaker.announces_focus() {
        s.app.row_label(s.app.cursor())
    } else {
        String::new()
    };
    let text = s.app.text(
        Template::AnnounceView,
        &[
            ("status", &s.app.status),
            ("label", &label),
            ("position", &(s.app.position() + 1).to_string()),
            ("count", &s.app.row_count().to_string()),
        ],
    );
    drop(s);
    gui.state.borrow_mut().speaker.announce(&text);
}

/// Handle a cursor move: push the new position into the native control, then (with no screen reader doing it for us) speak the row that landed under it.
fn moved(gui: &Gui) {
    // `suppress` is what makes it safe to hold this borrow across a call into wxWidgets: the selection events it fires synchronously turn around at the top of their handlers without touching `state`. Holding it rather than copying is what keeps a four-hundred-comment thread from cloning every one of its tree item handles on each keypress.
    gui.suppress.set(true);
    {
        let s = gui.state.borrow();
        match s.app.view {
            View::Stories | View::Help => select_list_row(&gui.list, s.app.cursor(), s.app.row_count()),
            View::Comments => select_tree_row(&gui.tree, &s.comment_items, s.app.cursor()),
            View::Settings => {}
        }
    }
    gui.suppress.set(false);

    sync_title(gui);

    let s = gui.state.borrow();
    if !s.speaker.announces_focus() {
        return;
    }
    let label = s.app.row_label(s.app.cursor());
    let text = s.app.text(
        Template::AnnounceRow,
        &[
            ("label", &label),
            ("position", &(s.app.position() + 1).to_string()),
            ("count", &s.app.row_count().to_string()),
        ],
    );
    drop(s);
    gui.state.borrow_mut().speaker.announce(&text);
}

/// Rebuild the list or tree from current state and select the current cursor, without speaking anything — callers that show a new view speak through `enter_view` instead.
fn populate_view(gui: &Gui) {
    let view = gui.state.borrow().app.view;
    gui.suppress.set(true);

    match view {
        View::Stories | View::Help => {
            gui.tree.show(false);
            gui.list.show(true);
            gui.list.delete_all_items();
            let count = gui.state.borrow().app.row_count();
            for i in 0..count {
                let label = gui.state.borrow().app.row_label(i);
                gui.list.insert_item(i as i64, &label, None);
            }
            let cursor = gui.state.borrow().app.cursor();
            select_list_row(&gui.list, cursor, count);
        }
        View::Comments => {
            gui.list.show(false);
            gui.tree.show(true);
            gui.tree.delete_all_items();

            let mut items: Vec<TreeItemId> = Vec::new();
            {
                let s = gui.state.borrow();
                if !s.app.comments.is_empty()
                    && let Some(root) = gui.tree.add_root("Comments", None, None)
                {
                    // The thread arrives flattened into reading order with a depth on each row; `stack` turns that back into nesting, which is what makes the tree announce reply level.
                    let mut stack: Vec<(i64, TreeItemId)> = Vec::new();
                    for (i, row) in s.app.comments.iter().enumerate() {
                        let depth = row.depth as i64;
                        while stack.last().is_some_and(|(d, _)| *d >= depth) {
                            stack.pop();
                        }
                        let parent = stack.last().map(|(_, id)| id).unwrap_or(&root);
                        let label = s.app.row_label(i);
                        if let Some(item) = gui.tree.append_item_with_data(parent, &label, i, None, None) {
                            items.push(item.clone());
                            stack.push((depth, item));
                        }
                    }
                    // Expand first so every reply has been laid out, then close the threads the user had closed: `collapse` on an item that was never expanded is a no-op, so the order matters when a view is rebuilt.
                    gui.tree.expand_all();
                    for (i, item) in items.iter().enumerate() {
                        if s.app.comment_is_collapsed(i) {
                            gui.tree.collapse(item);
                        }
                    }
                }
            }

            gui.state.borrow_mut().comment_items = items;
            let s = gui.state.borrow();
            let cursor = s.app.cursor();
            select_tree_row(&gui.tree, &s.comment_items, cursor);
        }
        View::Settings => {
            gui.list.show(false);
            gui.tree.show(false);
        }
    }

    // A hidden control is dropped from the sizer's reckoning, so this is what gives the one that was just shown the whole content area.
    gui.panel.layout();
    gui.suppress.set(false);
}

fn select_list_row(list: &ListCtrl, index: usize, count: usize) {
    let mask = ListItemState::Selected | ListItemState::Focused;
    for i in 0..count {
        let state = if i == index { mask } else { ListItemState::None };
        list.set_item_state(i as i64, state, mask);
    }
    if count > 0 {
        list.ensure_visible(index as i64);
        list.set_focus();
    }
}

/// Point the tree at one row. `index` is an index into the whole thread, so it is only ever a row that is actually showing — `ensure_visible` would otherwise reopen the very thread the user had just collapsed.
///
/// No `unselect_all` first: this is a single-selection tree, so selecting is already a replacement, and the moment with nothing selected that clearing leaves behind is a moment a screen reader has to say something about.
fn select_tree_row(tree: &TreeCtrl, items: &[TreeItemId], index: usize) {
    if let Some(item) = items.get(index) {
        tree.select_item(item);
        tree.ensure_visible(item);
        tree.set_focus();
    }
}

// ---- Actions ---------------------------------------------------------------

fn move_by(gui: &Gui, delta: isize) {
    let did_move = gui.state.borrow_mut().app.move_cursor(delta);
    if did_move {
        moved(gui);
    }
}

fn move_to(gui: &Gui, index: usize) {
    let did_move = gui.state.borrow_mut().app.move_to(index);
    if did_move {
        moved(gui);
    }
}

fn move_to_edge(gui: &Gui, last: bool) {
    let did_move = gui.state.borrow_mut().app.move_to_edge(last);
    if did_move {
        moved(gui);
    }
}

fn load_feed(gui: &Gui, feed: Feed) {
    let status = {
        let mut s = gui.state.borrow_mut();
        s.generation += 1;
        s.app.feed = feed;
        s.app.view = View::Stories;
        s.app.loading = true;
        let _ = s.requests.send(Request::Stories {
            generation: s.generation,
            feed,
        });
        s.app.text(Template::StatusLoadingFeed, &[("feed", &s.app.feed_title())])
    };
    set_status(gui, status);
    sync_menu_checks(gui);
}

fn open_comments(gui: &Gui) {
    let story = gui.state.borrow().app.selected_story().cloned();
    let Some(story) = story else {
        let status = gui.state.borrow().app.text(Template::StatusNothingSelected, &[]);
        set_status(gui, status);
        return;
    };

    let title = story.title.clone().unwrap_or_default();
    if story.kids.is_empty() {
        let status = gui.state.borrow().app.text(Template::StatusNoComments, &[("title", &title)]);
        set_status(gui, status);
        return;
    }

    let status = {
        let mut s = gui.state.borrow_mut();
        s.generation += 1;
        s.app.loading = true;
        let count = s.app.comments.len();
        let _ = s.requests.send(Request::Comments {
            generation: s.generation,
            story: Box::new(story.clone()),
        });
        s.app.comment_story = Some(story);
        s.app.text(
            Template::StatusLoadingComments,
            &[("title", &title), ("count", &count.to_string())],
        )
    };
    set_status(gui, status);
}

fn go_back(gui: &Gui) {
    let view = gui.state.borrow().app.view;
    match view {
        View::Settings => {}
        View::Help => {
            let title = {
                let mut s = gui.state.borrow_mut();
                s.app.view = s.app.previous_view;
                s.app.list_title()
            };
            enter_view(gui, title);
        }
        View::Comments => {
            let title = {
                let mut s = gui.state.borrow_mut();
                s.app.view = View::Stories;
                s.app.comments.clear();
                s.app.comment_cursor = 0;
                s.app.clear_comment_collapsed();
                s.app.comment_story = None;
                s.app.list_title()
            };
            enter_view(gui, title);
        }
        View::Stories => {
            let status = gui.state.borrow().app.text(Template::StatusAtTop, &[]);
            set_status(gui, status);
        }
    }
}

fn toggle_help(gui: &Gui) {
    let view = gui.state.borrow().app.view;
    if view == View::Help {
        go_back(gui);
        return;
    }
    let status = {
        let mut s = gui.state.borrow_mut();
        s.app.previous_view = s.app.view;
        s.app.view = View::Help;
        let output = s.speaker.status().to_string();
        s.app.text(Template::StatusHelp, &[("output", &output), ("count", &s.app.row_count().to_string())])
    };
    enter_view(gui, status);
}

/// Open a URL in the user's browser, reporting what happened either way.
fn open_url(gui: &Gui, url: Option<String>, what: Template) {
    let what_text = gui.state.borrow().app.text(what, &[]);
    let Some(url) = url else {
        let status = gui.state.borrow().app.text(Template::StatusNoLink, &[("what", &what_text)]);
        set_status(gui, status);
        return;
    };

    let status = match open::that_detached(&url) {
        Ok(()) => gui.state.borrow().app.text(Template::StatusOpened, &[("what", &what_text), ("url", &url)]),
        Err(err) => gui.state.borrow().app.text(
            Template::StatusOpenFailed,
            &[("what", &what_text), ("url", &url), ("error", &err.to_string())],
        ),
    };
    set_status(gui, status);
}

fn read_selection(gui: &Gui) {
    let detail = gui.state.borrow().app.selected_detail();
    say_on_demand(gui, detail);
}

/// Say something the user has explicitly asked to hear.
///
/// Unlike a status message this is not transient chatter, so it goes out whichever channel is listening: Prism when it has a backend, and the status line otherwise, where the screen reader picks it up instead.
fn say_on_demand(gui: &Gui, text: String) {
    let enabled = gui.state.borrow().speaker.is_enabled();
    if enabled {
        gui.state.borrow_mut().speaker.announce(&text);
    } else {
        set_status(gui, text);
    }
}

/// Left in the comment tree: close the thread under this comment, or, when there is nothing to close, step out to the comment it replies to.
///
/// This pairing is what every tree control on every platform does, and it is the whole point of showing comments as a tree: it is how a listener says "I am done with this subthread" and gets past it in one keystroke instead of forty.
fn collapse_current(gui: &Gui) -> bool {
    // Read everything under one borrow and let it go before acting: both branches below re-enter `state` mutably.
    let (index, open, parent) = {
        let s = gui.state.borrow();
        if s.app.view != View::Comments {
            return false;
        }
        let index = s.app.cursor();
        let open = s.app.comment_has_replies(index) && !s.app.comment_is_collapsed(index);
        (index, open, s.app.comment_parent(index))
    };

    if open {
        set_collapsed(gui, index, true);
    } else if let Some(parent) = parent {
        move_to(gui, parent);
    }
    true
}

/// Right in the comment tree: open the thread under this comment, or, when it is already open, step into the first reply.
fn expand_current(gui: &Gui) -> bool {
    let (index, closed) = {
        let s = gui.state.borrow();
        if s.app.view != View::Comments {
            return false;
        }
        let index = s.app.cursor();
        if !s.app.comment_has_replies(index) {
            return true;
        }
        (index, s.app.comment_is_collapsed(index))
    };

    if closed {
        set_collapsed(gui, index, false);
    } else {
        // The thread is flat and in reading order, so the first reply is simply the next row.
        move_to(gui, index + 1);
    }
    true
}

/// Hide or show one comment's replies, in the application's state and in the tree, and say so.
fn set_collapsed(gui: &Gui, index: usize, collapsed: bool) {
    if !gui.state.borrow_mut().app.set_comment_collapsed(index, collapsed) {
        return;
    }

    // The tree is the one doing the hiding; the state above only records it so that movement and the rebuilt view agree with what is on screen.
    gui.suppress.set(true);
    {
        let s = gui.state.borrow();
        if let Some(item) = s.comment_items.get(index) {
            if collapsed {
                gui.tree.collapse(item);
            } else {
                gui.tree.expand(item);
            }
        }
        // Closing a thread can have moved the cursor out of it.
        select_tree_row(&gui.tree, &s.comment_items, s.app.cursor());
    }
    gui.suppress.set(false);

    sync_title(gui);

    // A screen reader announces a tree item's own expanded/collapsed state, so this is only for the listener who has no screen reader to do it.
    let s = gui.state.borrow();
    if !s.speaker.announces_focus() {
        return;
    }
    let text = s.app.collapse_text(index, collapsed);
    drop(s);
    gui.state.borrow_mut().speaker.announce(&text);
}

/// The default action for the current row: comments for a story, and the permalink for a comment.
fn activate(gui: &Gui) {
    let view = gui.state.borrow().app.view;
    match view {
        View::Stories => open_comments(gui),
        View::Comments => {
            let url = gui.state.borrow().app.selected_item().map(|item| item.hn_url());
            open_url(gui, url, Template::WordComment);
        }
        View::Help => go_back(gui),
        View::Settings => {}
    }
}

fn reload(gui: &Gui) {
    let feed = gui.state.borrow().app.feed;
    load_feed(gui, feed);
}

fn toggle_speech(gui: &Gui) {
    let status = {
        let mut s = gui.state.borrow_mut();
        let on = s.speaker.toggle();
        let template = if on { Template::StatusSpeechOn } else { Template::StatusSpeechOff };
        let backend = s.speaker.status().to_string();
        s.app.text(template, &[("backend", &backend)])
    };
    set_status(gui, status);
    sync_menu_checks(gui);
}

/// Perform a command, however it was reached — the menu bar and the matching key both end up here, so the two can never drift apart.
fn run_command(gui: &Gui, command: Command) {
    match command {
        Command::SelectFeed(feed) => load_feed(gui, feed),
        Command::Reload => reload(gui),
        Command::OpenComments => open_comments(gui),
        Command::OpenLink => {
            let url = gui
                .state
                .borrow()
                .app
                .selected_item()
                .map(|item| item.url.clone().unwrap_or_else(|| item.hn_url()));
            open_url(gui, url, Template::WordLink);
        }
        Command::OpenDiscussion => {
            let url = gui.state.borrow().app.selected_story().map(|story| story.hn_url());
            open_url(gui, url, Template::WordDiscussion);
        }
        Command::ReadInFull => read_selection(gui),
        Command::StopSpeaking => gui.state.borrow_mut().speaker.stop(),
        Command::ToggleSpeech => toggle_speech(gui),
        Command::OpenSettings => open_settings(gui),
        Command::OpenHelp => toggle_help(gui),
        Command::Quit => gui.frame.close(true),
    }
}

// ---- Keyboard ---------------------------------------------------------------

/// Named (non-printable) keys, shared by the story/help list and the comment tree. Returns whether the key was consumed.
fn handle_named_key(gui: &Gui, code: i32) -> bool {
    if code == WXK_UP {
        move_by(gui, -1);
    } else if code == WXK_DOWN {
        move_by(gui, 1);
    } else if code == WXK_PAGEUP {
        move_by(gui, -PAGE);
    } else if code == WXK_PAGEDOWN {
        move_by(gui, PAGE);
    } else if code == WXK_HOME {
        move_to_edge(gui, false);
    } else if code == WXK_END {
        move_to_edge(gui, true);
    } else if code == WXK_LEFT {
        // Left and Right are the tree's own keys, and only the comment view has a tree. Everywhere else they stay the platform's to handle.
        return collapse_current(gui);
    } else if code == WXK_RIGHT {
        return expand_current(gui);
    } else if code == WXK_F1 {
        toggle_help(gui);
    } else {
        return false;
    }
    true
}

/// Printable characters, shared by the story/help list and the comment tree. Returns whether the character was consumed.
fn handle_char(gui: &Gui, ch: char) -> bool {
    if let Some(digit) = ch.to_digit(10) {
        let index = (digit as usize).wrapping_sub(1);
        if digit > 0 && index < Feed::ALL.len() {
            load_feed(gui, Feed::ALL[index]);
        }
        return true;
    }

    match ch {
        '\u{8}' => go_back(gui), // Backspace
        '\r' | '\n' => activate(gui),
        '\u{1b}' => {
            // Escape: at the story list either quits or is just Backspace's synonym for "go back", depending on the preference; elsewhere it always means "go back".
            let (view, escape_exits) = {
                let s = gui.state.borrow();
                (s.app.view, s.app.preferences.escape_exits)
            };
            if view == View::Stories && escape_exits {
                gui.frame.close(true);
            } else {
                go_back(gui);
            }
        }
        'j' | 'J' => move_by(gui, 1),
        'k' | 'K' => move_by(gui, -1),
        'o' | 'O' => {
            let url = gui
                .state
                .borrow()
                .app
                .selected_item()
                .map(|item| item.url.clone().unwrap_or_else(|| item.hn_url()));
            open_url(gui, url, Template::WordLink);
        }
        'c' | 'C' => {
            let url = gui.state.borrow().app.selected_story().map(|story| story.hn_url());
            open_url(gui, url, Template::WordDiscussion);
        }
        'r' | 'R' => reload(gui),
        'p' | 'P' => read_selection(gui),
        's' | 'S' => gui.state.borrow_mut().speaker.stop(),
        'v' | 'V' => toggle_speech(gui),
        ',' => open_settings(gui),
        'h' | 'H' => toggle_help(gui),
        'q' | 'Q' => gui.frame.close(true),
        _ => return false,
    }
    true
}

/// Bind the global single-letter/movement commands to a content control (the story/help list or the comment tree). Both get identical bindings so the commands work no matter which one currently holds focus.
fn bind_content_keys<W: WindowEvents>(widget: &W, gui: &Gui) {
    let g = gui.clone();
    widget.on_key_down(move |e: WindowEventData| {
        let handled = matches!(&e, WindowEventData::Keyboard(kb) if kb.get_key_code().is_some_and(|code| handle_named_key(&g, code)));
        // A bound handler that does not skip swallows the key. Anything we did not act on has to go on to the control and then to wxWidgets itself, or Tab would not move focus, Alt would not open the menu bar, and — since a character event only follows a key-down that was skipped — `on_char` below would never run at all.
        e.skip(!handled);
    });

    let g = gui.clone();
    widget.on_char(move |e: WindowEventData| {
        let handled = matches!(&e, WindowEventData::Keyboard(kb)
            if kb.get_unicode_key().and_then(|cp| char::from_u32(cp as u32)).is_some_and(|ch| handle_char(&g, ch)));
        e.skip(!handled);
    });
}

/// Mouse-driven (or otherwise externally driven) focus/selection changes on the content controls: clicking a row, or a screen reader's own virtual cursor landing on one.
fn bind_content_selection(gui: &Gui) {
    let g = gui.clone();
    gui.list.on_item_focused(move |e: ListCtrlEventData| {
        if g.suppress.get() {
            return;
        }
        let index = e.get_item_index();
        if index >= 0 {
            move_to(&g, index as usize);
        }
    });
    let g = gui.clone();
    gui.list.on_item_activated(move |_| activate(&g));

    let g = gui.clone();
    gui.tree.on_selection_changed(move |e: TreeEventData| {
        if g.suppress.get() {
            return;
        }
        let Some(index) = e.get_item().and_then(|item| tree_row(&g, &item)) else {
            return;
        };
        move_to(&g, index);
    });
    let g = gui.clone();
    gui.tree.on_item_activated(move |_| activate(&g));

    // Clicking the expander is the other way to open and close a thread, and the cursor has to step over the same rows afterwards however it was done. `suppress` is what keeps this from echoing back the collapses this application asked the tree for itself.
    let g = gui.clone();
    gui.tree.on_item_collapsed(move |e: TreeEventData| collapsed_in_tree(&g, e, true));
    let g = gui.clone();
    gui.tree.on_item_expanded(move |e: TreeEventData| collapsed_in_tree(&g, e, false));
}

/// Record a thread the tree itself opened or closed, so movement agrees with what is on screen.
fn collapsed_in_tree(gui: &Gui, e: TreeEventData, collapsed: bool) {
    if gui.suppress.get() {
        return;
    }
    let Some(index) = e.get_item().and_then(|item| tree_row(gui, &item)) else {
        return;
    };
    if gui.state.borrow_mut().app.set_comment_collapsed(index, collapsed) {
        sync_title(gui);
    }
}

/// The thread row a tree item stands for. Every item carries its index into `app.comments` as its data, which is the only thing that survives the tree being rebuilt.
fn tree_row(gui: &Gui, item: &TreeItemId) -> Option<usize> {
    gui.tree
        .get_custom_data(item)
        .and_then(|data| data.downcast_ref::<usize>().copied())
}

/// The row-relevant context menu: right-click, or the keyboard's Menu key / Shift+F10, either of which wxWidgets reports as `wxEVT_CONTEXT_MENU` on whichever of `list` or `tree` is focused. That event bubbles up to `panel` (their common wx parent) when neither handles it, so one binding here covers both controls without needing a copy per widget.
fn bind_context_menu(gui: &Gui) {
    let g = gui.clone();
    gui.panel.on_context_menu(move |e: MenuEventData| {
        let view = g.state.borrow().app.view;
        if !matches!(view, View::Stories | View::Comments) {
            e.skip(true);
            return;
        }
        let mut menu = build_context_menu(&g.state.borrow().app);
        g.panel.popup_menu(&mut menu, e.get_context_position());
    });
}

// ---- The menu bar -----------------------------------------------------------

/// Build the menu bar. Takes the application because one command — choosing a feed — is labelled with a user-editable template rather than a fixed word, and wxWidgets has no use for a menu item with no text on it.
fn build_menu_bar(app: &App) -> MenuBar {
    let mut builder = MenuBar::builder();
    for bar in menu::BARS {
        let menu_builder = append_items(Menu::builder(), app, bar.items);
        builder = builder.append(menu_builder.build(), bar.name);
    }
    builder.build()
}

/// Append a menu's entries to a builder, wired to the same `Command` ids the menu bar and the keyboard use — shared so a popup menu built from the same items can never drift from the menu bar's own.
fn append_items(mut builder: MenuBuilder, app: &App, items: &[MenuEntry]) -> MenuBuilder {
    for item in items {
        builder = match item {
            MenuEntry::Separator => builder.append_separator(),
            MenuEntry::Entry(command) => {
                let label = app.command_label(*command);
                match command.kind() {
                    Kind::Normal => builder.append_item(command.id(), &label, ""),
                    Kind::Checkbox => builder.append_check_item(command.id(), &label, ""),
                    Kind::Radio => builder.append_radio_item(command.id(), &label, ""),
                }
            }
        };
    }
    builder
}

/// The right-click / Menu-key context menu for the story and comment lists: the same "Story" commands the menu bar offers, reachable without leaving the row under the cursor. Built fresh each time so a user-edited feed template or other label change can never leave it stale.
fn build_context_menu(app: &App) -> Menu {
    let items = menu::BARS
        .iter()
        .find(|bar| bar.name == "Story")
        .map(|bar| bar.items)
        .unwrap_or(&[]);
    append_items(Menu::builder(), app, items).build()
}

/// `SelectFeed`'s label is the feed's own user-editable name, so unlike every other command it can go stale when a template changes; refreshed here at startup and whenever the settings dialog closes.
fn sync_feed_labels(gui: &Gui) {
    let s = gui.state.borrow();
    for feed in Feed::ALL {
        let command = Command::SelectFeed(feed);
        if let Some(item) = gui.menu_bar.find_item(command.id()) {
            item.set_label(&s.app.command_label(command));
        }
    }
}

/// Refresh which feed reads as current and whether speech reads as on, after either changes.
fn sync_menu_checks(gui: &Gui) {
    let s = gui.state.borrow();
    for bar in menu::BARS {
        for item in bar.items {
            if let MenuEntry::Entry(command) = item
                && command.kind() != Kind::Normal
            {
                let marked = s.app.command_marked(*command, s.speaker.is_enabled());
                gui.menu_bar.check_item(command.id(), marked);
            }
        }
    }
}

fn bind_menu_events(gui: &Gui) {
    let g = gui.clone();
    gui.frame.on_menu_selected(move |e: MenuEventData| {
        if let Some(command) = Command::from_id(e.get_id()) {
            run_command(&g, command);
        }
    });

    let g = gui.clone();
    gui.frame.on_menu_highlighted(move |e: MenuEventData| {
        let Some(command) = Command::from_id(e.get_id()) else { return };
        let s = g.state.borrow();
        if !s.speaker.announces_focus() {
            return;
        }
        let speech_active = s.speaker.is_enabled();
        let (index, count) = match menu::position_of(command) {
            Some((index, count)) => (index.to_string(), count.to_string()),
            None => (String::new(), String::new()),
        };
        let text = s.app.text(
            Template::MenuItemFocus,
            &[
                ("name", &s.app.command_label(command)),
                ("state", &s.app.command_state_text(command, speech_active)),
                ("index", &index),
                ("count", &count),
            ],
        );
        drop(s);
        g.state.borrow_mut().speaker.announce(&text);
    });

    let g = gui.clone();
    gui.frame.on_menu_opened(move |_| {
        let s = g.state.borrow();
        if !s.speaker.announces_focus() {
            return;
        }
        let text = s.app.text(Template::MenuOpened, &[("label", "")]);
        drop(s);
        g.state.borrow_mut().speaker.announce(&text);
    });
}

// ---- The settings dialog ----------------------------------------------------

fn open_settings(gui: &Gui) {
    {
        let mut s = gui.state.borrow_mut();
        if s.app.view == View::Settings {
            return;
        }
        // Entering from help, keep the view help itself would return to rather than building a loop between the two.
        if s.app.view != View::Help {
            s.app.previous_view = s.app.view;
        }
        s.app.view = View::Settings;
        s.app.settings.open();
    }

    let title = gui.state.borrow().app.list_title();
    {
        let s = gui.state.borrow();
        if s.speaker.announces_focus() {
            let intro = s.app.text(
                Template::SettingsIntro,
                &[
                    ("tab", s.app.settings.tab_name()),
                    ("index", &(s.app.settings.tab() + 1).to_string()),
                    ("count", &TABS.len().to_string()),
                ],
            );
            drop(s);
            gui.state.borrow_mut().speaker.announce(&intro);
        }
    }

    let dialog = Dialog::builder(&gui.frame, &title)
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(760, 520)
        .build();

    let notebook = Notebook::builder(&dialog).build();
    for (tab_index, tab) in TABS.iter().enumerate() {
        let page = build_settings_page(&notebook, tab_index, gui);
        // Even the name on a tab is a template, like every other word this application shows or says.
        let label = gui.state.borrow().app.text(
            Template::SettingsTabLabel,
            &[
                ("name", tab.name),
                ("index", &(tab_index + 1).to_string()),
                ("count", &TABS.len().to_string()),
            ],
        );
        notebook.add_page(&page, &label, tab_index == 0, None);
    }
    {
        let g = gui.clone();
        notebook.on_page_changed(move |e: NotebookPageChangedEvent| {
            let Some(selection) = e.get_selection() else { return };
            g.state.borrow_mut().app.settings.select_tab(selection as usize);

            let s = g.state.borrow();
            if !s.speaker.announces_focus() {
                return;
            }
            let text = s.app.text(
                Template::SettingsTabFocus,
                &[
                    ("name", s.app.settings.tab_name()),
                    ("index", &(s.app.settings.tab() + 1).to_string()),
                    ("count", &TABS.len().to_string()),
                ],
            );
            drop(s);
            g.state.borrow_mut().speaker.announce(&text);
        });
    }

    let close_button = Button::builder(&dialog).with_label("Close").build();
    close_button.on_click(move |_| dialog.end_modal(ID_OK));

    // wxWidgets closes a dialog on Escape by itself in most cases; saying so here makes it true in all of them, and costs one comparison.
    dialog.on_key_down(move |e: WindowEventData| {
        let escape = matches!(&e, WindowEventData::Keyboard(kb) if kb.get_key_code() == Some(WXK_ESCAPE));
        if escape {
            dialog.end_modal(ID_CANCEL);
        }
        e.skip(!escape);
    });

    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&notebook, 1, SizerFlag::Expand | SizerFlag::All, 8);
    dialog_sizer.add(&close_button, 0, SizerFlag::AlignRight | SizerFlag::All, 8);
    dialog.set_sizer(dialog_sizer, true);

    dialog.show_modal();
    dialog.destroy();

    close_settings(gui);
}

/// One tab's worth of the settings dialog: a grouped tree of its fields next to a single editor pane (a `TextCtrl`, or for the one non-text field a `CheckBox`) that shows whichever field is currently selected in the tree.
fn build_settings_page(parent: &Notebook, tab_index: usize, gui: &Gui) -> Panel {
    let panel = Panel::builder(parent).build();
    let main_sizer = BoxSizer::builder(Orientation::Horizontal).build();

    let tree = TreeCtrl::builder(&panel)
        .with_style(TreeCtrlStyle::HasButtons | TreeCtrlStyle::LinesAtRoot | TreeCtrlStyle::HideRoot | TreeCtrlStyle::Single)
        .build();

    let editor_panel = Panel::builder(&panel).build();
    let editor_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let description = StaticText::builder(&editor_panel).with_label("").build();
    let text_ctrl = TextCtrl::builder(&editor_panel).with_style(TextCtrlStyle::MultiLine).build();
    let checkbox = CheckBox::builder(&editor_panel)
        .with_label(Field::EscapeExits.label())
        .build();
    checkbox.show(false);
    editor_sizer.add(&description, 0, SizerFlag::Expand | SizerFlag::All, 6);
    editor_sizer.add(&text_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 6);
    editor_sizer.add(&checkbox, 0, SizerFlag::Expand | SizerFlag::All, 6);
    editor_panel.set_sizer(editor_sizer, true);

    main_sizer.add(&tree, 1, SizerFlag::Expand | SizerFlag::All, 6);
    main_sizer.add(&editor_panel, 1, SizerFlag::Expand | SizerFlag::All, 6);
    panel.set_sizer(main_sizer, true);

    let fields = fields_of(tab_index);
    if let Some(root) = tree.add_root("Fields", None, None) {
        let s = gui.state.borrow();
        for (group, range) in groups(&fields) {
            let group_label = s.app.text(
                Template::SettingsGroupLabel,
                &[("name", group.name()), ("count", &range.len().to_string())],
            );
            let Some(group_item) = tree.append_item(&root, &group_label, None, None) else {
                continue;
            };
            for field_index in range {
                let label = s.app.field_label_text(fields[field_index], field_index, fields.len());
                tree.append_item_with_data(&group_item, &label, field_index, None, None);
            }
        }
        tree.expand_all();
    }

    {
        let g = gui.clone();
        tree.on_selection_changed(move |e: TreeEventData| {
            let Some(item) = e.get_item() else { return };
            let field_index = tree.get_custom_data(&item).and_then(|d| d.downcast_ref::<usize>().copied());
            let Some(field_index) = field_index else {
                text_ctrl.show(false);
                checkbox.show(false);
                description.set_label("");
                return;
            };

            g.state.borrow_mut().app.settings.select_field(field_index);

            let (field, help_text, is_toggle, announces) = {
                let s = g.state.borrow();
                let field = s.app.settings.fields()[field_index];
                let count = s.app.settings.fields().len();
                (
                    field,
                    s.app.field_help_text(field, field_index, count),
                    s.app.settings.is_toggle(),
                    s.speaker.announces_focus(),
                )
            };
            description.set_label(&help_text);

            if is_toggle {
                let value = g.state.borrow().app.preferences.escape_exits;
                text_ctrl.show(false);
                checkbox.set_value(value);
                checkbox.show(true);
                checkbox.set_focus();
            } else if let Field::Template(template) = field {
                let value = g.state.borrow().app.templates.get(template).to_string();
                checkbox.show(false);
                text_ctrl.set_value(&value);
                text_ctrl.show(true);
                text_ctrl.set_focus();
            }
            editor_panel.layout();

            if announces {
                let text = g.state.borrow().app.field_focus_text(field_index);
                g.state.borrow_mut().speaker.announce(&text);
            }
        });
    }

    {
        let g = gui.clone();
        text_ctrl.on_text_updated(move |e: TextEventData| {
            let field = g.state.borrow().app.settings.focused_field();
            if let Some(Field::Template(template)) = field
                && let Some(value) = e.get_string()
            {
                g.state.borrow_mut().app.templates.set(template, value);
            }
        });
    }

    {
        let g = gui.clone();
        checkbox.on_toggled(move |e: CheckBoxEventData| {
            g.state.borrow_mut().app.preferences.escape_exits = e.is_checked();
        });
    }

    bind_settings_keys(&tree, gui, text_ctrl);
    bind_settings_keys(&text_ctrl, gui, text_ctrl);
    bind_settings_keys(&checkbox, gui, text_ctrl);

    panel
}

/// The two keys the settings dialog adds to what wxWidgets already does for it: F1 for the key list, F5 to restore a field's default.
///
/// Bound to every control the dialog can put focus on, because picking a field in the tree moves focus into the editor — binding the tree alone would put F5 out of reach exactly when a user wants it.
fn bind_settings_keys<W: WindowEvents>(widget: &W, gui: &Gui, text_ctrl: TextCtrl) {
    let g = gui.clone();
    widget.on_key_down(move |e: WindowEventData| {
        let code = if let WindowEventData::Keyboard(kb) = &e { kb.get_key_code() } else { None };
        match code {
            Some(WXK_F1) => speak_settings_keys(&g),
            Some(WXK_F5) => reset_focused_field(&g, &text_ctrl),
            // Everything else is the dialog's own: the tree moves between fields, the editor edits, Tab moves between the two.
            _ => {
                e.skip(true);
                return;
            }
        }
        e.skip(false);
    });
}

fn speak_settings_keys(gui: &Gui) {
    let s = gui.state.borrow();
    let text = s.app.text(
        Template::SettingsKeys,
        &[
            ("tab", s.app.settings.tab_name()),
            ("index", &(s.app.settings.tab() + 1).to_string()),
            ("count", &TABS.len().to_string()),
        ],
    );
    drop(s);
    say_on_demand(gui, text);
}

/// Put the selected field back to its compiled-in default, in the model and in the editor showing it. A no-op on the checkbox, which has no text.
fn reset_focused_field(gui: &Gui, text_ctrl: &TextCtrl) {
    let reset = {
        let mut state = gui.state.borrow_mut();
        let app = &mut state.app;
        app.settings.reset_field(&mut app.templates)
    };
    let Some(field) = reset else { return };

    let (value, status) = {
        let s = gui.state.borrow();
        (
            s.app.templates.get(field).to_string(),
            s.app.text(
                Template::TemplateReset,
                &[("name", field.label()), ("default", field.default_text())],
            ),
        )
    };
    text_ctrl.set_value(&value);
    set_status(gui, status);
}

/// Leave the dialog, writing the edits to disk.
///
/// A template that will not render as its author intended is reported in preference to the save itself: the file being written matters less than the user knowing that one of their announcements is broken. It is saved either way — their text is theirs, mistake or not.
fn close_settings(gui: &Gui) {
    let (saved, problem, previous) = {
        let s = gui.state.borrow();
        let saved = config::save(&s.app.templates).and_then(|path| {
            preferences::save(&s.app.preferences)?;
            Ok(path)
        });
        let problem = s
            .app
            .settings
            .fields()
            .iter()
            .find_map(|field| {
                let Field::Template(template) = field else {
                    return None;
                };
                validate(*template, s.app.templates.get(*template)).map(|problem| (field.label(), problem))
            })
            .map(|(label, problem)| s.app.text(Template::TemplateInvalid, &[("name", label), ("problem", &problem)]));
        (saved, problem, s.app.previous_view)
    };

    let status = match (saved, problem) {
        (Err(error), _) => gui.state.borrow().app.text(Template::SettingsSaveFailed, &[("error", &error)]),
        (Ok(_), Some(problem)) => problem,
        (Ok(path), None) => gui
            .state
            .borrow()
            .app
            .text(Template::SettingsSaved, &[("path", &path.display().to_string())]),
    };

    gui.state.borrow_mut().app.view = previous;
    sync_feed_labels(gui);
    enter_view(gui, status);
}

// ---- Background work --------------------------------------------------------

/// Run network work off the GUI thread, reporting back through a channel polled from an idle handler (see `bind_worker_idle`) — the GUI's own state lives behind an `Rc`, which cannot cross threads, so the worker only ever produces plain `Send` data.
fn spawn_worker() -> (mpsc::Sender<Request>, mpsc::Receiver<WorkResult>) {
    let (request_tx, request_rx) = mpsc::channel::<Request>();
    let (result_tx, result_rx) = mpsc::channel::<WorkResult>();

    thread::spawn(move || {
        let client = hn::Client::new();
        for request in request_rx {
            let sent = match request {
                Request::Stories { generation, feed } => {
                    let result = client.story_ids(feed, STORY_LIMIT).map(|ids| client.items(&ids));
                    result_tx.send(WorkResult::Stories { generation, feed, result })
                }
                Request::Comments { generation, story } => {
                    let rows = client.comment_thread(&story.kids, COMMENT_LIMIT);
                    // An empty result for a story that advertises comments means the fetch failed, not that the thread is empty.
                    let result = if rows.is_empty() && !story.kids.is_empty() {
                        Err("no comments could be fetched".to_string())
                    } else {
                        Ok(rows)
                    };
                    result_tx.send(WorkResult::Comments { generation, result })
                }
            };
            if sent.is_err() {
                break; // The GUI is gone; so are we.
            }
            // The GUI thread is asleep in the event loop and nothing it can see has changed, so poke it: this is the one wx call that is safe from another thread, and it is what makes the idle handler in `bind_worker_idle` run and pick the result up.
            wxdragon::wake_up_idle();
        }
    });

    (request_tx, result_rx)
}

fn bind_worker_idle(gui: &Gui, result_rx: mpsc::Receiver<WorkResult>) {
    let g = gui.clone();
    gui.frame.on_idle(move |_| {
        while let Ok(result) = result_rx.try_recv() {
            apply_result(&g, result);
        }
    });
}

fn apply_result(gui: &Gui, result: WorkResult) {
    match result {
        WorkResult::Stories { generation, feed, result } => {
            if generation != gui.state.borrow().generation {
                return; // Superseded by newer navigation.
            }
            gui.state.borrow_mut().app.loading = false;
            let feed_title = gui.state.borrow().app.templates.feed_title(feed);
            match result {
                Ok(stories) => {
                    let count = stories.len();
                    {
                        let mut s = gui.state.borrow_mut();
                        s.app.stories = stories;
                        s.app.story_cursor = 0;
                        s.app.view = View::Stories;
                    }
                    let status = gui
                        .state
                        .borrow()
                        .app
                        .text(Template::StatusFeedLoaded, &[("feed", &feed_title), ("count", &count.to_string())]);
                    enter_view(gui, status);
                }
                Err(err) => {
                    let status = gui
                        .state
                        .borrow()
                        .app
                        .text(Template::StatusFeedError, &[("feed", &feed_title), ("error", &err)]);
                    set_status(gui, status);
                }
            }
        }

        WorkResult::Comments { generation, result } => {
            if generation != gui.state.borrow().generation {
                return;
            }
            gui.state.borrow_mut().app.loading = false;
            let title = gui
                .state
                .borrow()
                .app
                .comment_story
                .as_ref()
                .and_then(|story| story.title.clone())
                .unwrap_or_default();

            match result {
                Ok(comments) => {
                    let count = comments.len();
                    {
                        let mut s = gui.state.borrow_mut();
                        s.app.comments = comments;
                        s.app.comment_cursor = 0;
                        // Collapsed rows are indices into the thread that was showing, so they mean nothing against a new one.
                        s.app.clear_comment_collapsed();
                        s.app.view = View::Comments;
                    }
                    let status = gui
                        .state
                        .borrow()
                        .app
                        .text(Template::StatusCommentsLoaded, &[("count", &count.to_string()), ("title", &title)]);
                    enter_view(gui, status);
                }
                Err(err) => {
                    gui.state.borrow_mut().app.comment_story = None;
                    let status = gui
                        .state
                        .borrow()
                        .app
                        .text(Template::StatusCommentsError, &[("error", &err), ("title", &title)]);
                    set_status(gui, status);
                }
            }
        }
    }
}
